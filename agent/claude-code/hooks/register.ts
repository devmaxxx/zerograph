import { atom, read, update } from 'claude-code'
import type { Hook, Register } from 'claude-code'

import { lineOf, readRun, type Report } from './parse'

// `status` walks the tree to count the files behind, which is real work on a large repository.
const POLL_MS = 10_000
const SHOWN_LINES = 15

const report = atom({ plugin: 'zerograph', key: 'report' } as const, null)

type Api = Parameters<Hook<'session.start'>>[0]

// The configured binary, else where a project keeps one: on PATH, through pnpm's bin
// directory (how a JS repository pins it), or built in place (zerograph's own checkout).
async function candidates($: Api, configured: string): Promise<string[]> {
  if (configured) return [configured]
  const root = await $.session.root()
  const local = [`${root}/node_modules/.bin/zerograph`, `${root}/target/release/zerograph`, `${root}/target/debug/zerograph`]
  const present = await Promise.all(local.map(p => $.fs.exists(p)))
  return ['zerograph', ...local.filter((_, i) => present[i])]
}

// `undefined` when no zerograph found has `status`, or each failed.
async function runStatus($: Api, configured: string, json: boolean): Promise<string | undefined> {
  const cwd = await $.session.root()
  for (const bin of await candidates($, configured)) {
    const argv = json ? [bin, 'status', '--json'] : [bin, 'status']
    const ran = await $.process.run(argv, { cwd, timeoutMs: 20_000 }).catch(() => undefined)
    if (ran?.exitCode === 0 && ran.stdout.trim()) return ran.stdout
  }
  return undefined
}

// A store built before the rename still sits under `.repograph` until a write moves it.
async function storeDir($: Api): Promise<string | undefined> {
  const root = await $.session.root()
  for (const name of ['.zerograph', '.repograph']) {
    if (await $.fs.exists(`${root}/${name}`)) return `${root}/${name}`
  }
  return undefined
}

async function fromLogs($: Api) {
  const dir = await storeDir($)
  if (!dir) return { isServing: false }
  const log = `${dir}/background.log`
  const isServing = await $.fs.exists(`${dir}/serve.sock`)
  if (!(await $.fs.exists(log))) return { dir, isServing }
  const [text, stat, now] = await Promise.all([$.fs.read(log), $.fs.stat(log), $.clock.now()])
  return { dir, run: readRun(text, stat.mtimeMs, now), isServing }
}

async function poll($: Api, configured: string): Promise<void> {
  const json = await runStatus($, configured, true)
  if (json) {
    const r = JSON.parse(json) as Report
    await update($, report, () => r)
    return
  }
  await update($, report, () => null)
}

export const register: Register = (on, options) => {
  const configured = typeof options.binary === 'string' ? options.binary.trim() : ''

  on('session.start', async ($, e, next) => {
    await $.command.register({
      name: 'zerograph',
      description: 'Show zerograph index, refresh progress, serve state and warnings',
    })
    const tick = () => poll($, configured).catch(() => update($, report, () => null))
    void tick()
    $.clock.every(POLL_MS, tick)

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
    if (text) return { text: text.trimEnd() }

    const { dir, run, isServing } = await fromLogs($)
    if (!dir) return { text: 'zerograph: no store in this project; `zerograph build` makes one.' }
    const out = [`store: ${dir}`, `serve: ${isServing ? 'running (socket present)' : 'not running'}`]
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
