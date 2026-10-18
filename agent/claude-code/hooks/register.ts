import { atom, read, update } from 'claude-code'
import type { Hook, Register } from 'claude-code'

import { isFullDue, lineOf, overlay, readRun, socketOf, type LiveLog, type Report } from './parse'

// A read of the log and a stat of the socket, cheap enough to keep the line live in every session.
const POLL_MS = 10_000
const SHOWN_LINES = 15

const report = atom({ plugin: 'zerograph', key: 'report' } as const, null)
// Kept apart from what the line draws, so each tick lays the live state over `status`'s own words.
const full = atom({ plugin: 'zerograph', key: 'full' } as const, null)

type Api = Parameters<Hook<'session.start'>>[0]

// The configured binary, else PATH's, and never one under the checkout: a cloned repository that
// ships an executable `node_modules/.bin/zerograph` or `target/*/zerograph` would have it run the
// moment a session opened in it. A project-local install is named in the `binary` option.
// `undefined` when that zerograph has no `status`, or it failed.
async function runStatus($: Api, configured: string, json: boolean): Promise<string | undefined> {
  const cwd = await $.session.root()
  const bin = configured || 'zerograph'
  const argv = json ? [bin, 'status', '--json'] : [bin, 'status']
  const ran = await $.process.run(argv, { cwd, timeoutMs: 20_000 }).catch(() => undefined)
  return ran?.exitCode === 0 && ran.stdout.trim() ? ran.stdout : undefined
}

// A store built before the rename still sits under `.repograph` until a write moves it.
async function storeDir($: Api): Promise<string | undefined> {
  const root = await $.session.root()
  for (const name of ['.zerograph', '.repograph']) {
    if (await $.fs.exists(`${root}/${name}`)) return `${root}/${name}`
  }
  return undefined
}

// A log untouched since `since` is not read: the overlay keeps the full report's reading of it.
async function readLog($: Api, dir: string, since = -Infinity): Promise<LiveLog | undefined> {
  const path = `${dir}/background.log`
  const stat = await $.fs.stat(path).catch(() => undefined)
  if (!stat) return undefined
  if (stat.mtimeMs <= since) return { mtimeMs: stat.mtimeMs, run: undefined }
  // A log the read refuses (over 4 MiB, or a link to a device) must not stop the tick before
  // the full report that would read it properly.
  const text = await $.fs.read(path).catch(() => undefined)
  if (text === undefined) return { mtimeMs: stat.mtimeMs, run: undefined }
  return { mtimeMs: stat.mtimeMs, run: readRun(text, stat.mtimeMs, await $.clock.now()) }
}

async function fromLogs($: Api) {
  const dir = await storeDir($)
  if (!dir) return {}
  const socket = socketOf((await read($, full))?.report, dir) ?? `${dir}/serve.sock`
  const [log, hasSocket] = await Promise.all([readLog($, dir), $.fs.exists(socket)])
  return { dir, run: log?.run, socket, hasSocket }
}

function reportOf(json: string | undefined): Report | null {
  if (!json) return null
  try {
    return JSON.parse(json) as Report
  } catch {
    return null
  }
}

async function takeFull($: Api, configured: string): Promise<void> {
  // Before `status` runs, so a socket bound or a line logged meanwhile reads as news to the overlay.
  const takenAt = await $.clock.now()
  const r = reportOf(await runStatus($, configured, true))
  // Recorded even with no report, so a zerograph without `status` is asked again at the interval.
  await update($, full, () => ({ report: r, takenAt }))
  await update($, report, () => r)
}

// A tick that finds a full report under way skips: it would lay the live state over the one
// being replaced.
let isTaking = false

async function tick($: Api, configured: string, isForced = false): Promise<void> {
  if (isTaking) return
  const [last, dir, now] = await Promise.all([read($, full), storeDir($), $.clock.now()])
  const log = dir ? await readLog($, dir, last?.takenAt) : undefined
  if (isForced || isFullDue(last?.takenAt, now, log)) {
    if (isTaking) return
    isTaking = true
    try {
      await takeFull($, configured)
    } finally {
      isTaking = false
    }
    return
  }
  if (!last?.report) return
  const path = socketOf(last.report, dir)
  const socket = path ? await $.fs.stat(path).then(s => ({ mtimeMs: s.mtimeMs }), () => undefined) : undefined
  const live = overlay({ report: last.report, takenAt: last.takenAt }, { log, socket })
  // A full report taken while this tick read the files is newer than what the overlay was laid on.
  if (isTaking || (await read($, full))?.takenAt !== last.takenAt) return
  await update($, report, () => live)
}

export const register: Register = (on, options) => {
  const configured = typeof options.binary === 'string' ? options.binary.trim() : ''

  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'zerograph',
      description: 'Show zerograph index, refresh progress, serve state and warnings',
    })
    void tick($, configured, true).catch(() => update($, report, () => null))
    $.clock.every(POLL_MS, () => tick($, configured).catch(() => update($, report, () => null)))

    return next(e)
  })

  // The line under the prompt carries zerograph's state at its end; the engine draws it dim and
  // cuts it where the row ends.
  on('ui.render', { component: 'PromptHint' }, async ($, e, next) => {
    const r = await read($, report)
    if (r === null) return next(e)
    return next({ ...e, props: { ...e.props, tail: lineOf(r) } })
  })

  on('command.run', { command: 'zerograph' }, async $ => {
    const text = await runStatus($, configured, false)
    // Whoever asks for the report wants the line to agree with it.
    void tick($, configured, true).catch(() => undefined)
    if (text) return { text: text.trimEnd() }

    const { dir, run, socket, hasSocket } = await fromLogs($)
    if (!dir) return { text: 'zerograph: no store in this project; `zerograph build` makes one.' }
    // A file at the path is all `$` can see: one a killed server left looks the same.
    const out = [`store: ${dir}`, `serve: ${hasSocket ? `a socket is at ${socket}, not asked whether it answers` : `not running (no socket at ${socket})`}`]
    if (!run) {
      out.push('refresh: no run logged')
    } else {
      out.push(`refresh: ${run.isRunning ? 'running' : 'last run ended'} — ${run.meter ?? run.last ?? 'no output yet'}`)
      out.push(run.warnings.length ? `warnings:\n${run.warnings.map(w => `  ${w}`).join('\n')}` : 'warnings: none')
      out.push(`log tail:\n${run.lines.slice(-SHOWN_LINES).map(l => `  ${l}`).join('\n')}`)
    }
    out.push('(this zerograph has no `status` command; upgrade for the full report)')

    return { text: out.join('\n') }
  })
}
