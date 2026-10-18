// Mirrors zerograph's own reading of `background.log`: the log is kept across runs, so only the
// lines after the last start marker belong to the run in hand.
export const RUN_MARKER = 'refresh: started'

// Fallback for a zerograph without `status`: a refresh writes a meter every few seconds while it
// embeds, so a log untouched for longer is a run that ended or was killed.
export const QUIET_MS = 60_000

const WARNING = /\b(warning|unreadable|skipped|failed|refused|error|cannot|could not|not read)\b/i

import type { Report } from '../types'

export type { Report }

const isMeter = (line: string) => line.startsWith('embedded ')

export function warningsOf(lines: string[]): string[] {
  return lines.filter(l => !isMeter(l) && WARNING.test(l))
}

const BAR = 8

// "embedded 32/40 (8.0 rows/s, ~1 s left)" reads as a bar, a percentage and the time left.
function meterOf(progress: string): string {
  const m = /^embedded (\d+)\/(\d+) \([^,]*, ~?([^)]*?) left\)/.exec(progress)
  if (!m) return progress
  const done = Number(m[1]) / Math.max(Number(m[2]), 1)
  const filled = Math.min(BAR, Math.round(done * BAR))
  return `${'▰'.repeat(filled)}${'▱'.repeat(BAR - filled)} ${Math.floor(done * 100)}% · ${(m[3] ?? '').replace(' ', '')} left`
}

// The hint line's tail, most pressing first since the engine cuts it where the row ends:
// a filled diamond while a refresh works, a hollow one at rest.
export function lineOf(r: Report): string {
  if (r.index.state === 'empty') return '◇ zerograph · no index'
  if (r.index.state === 'unreadable') return '◇ zerograph · ⚠ index unreadable'
  const isWorking = r.refresh.state === 'running' || r.refresh.state === 'starting'
  const parts = [isWorking ? '◆ zerograph' : '◇ zerograph']
  if (r.refresh.state === 'running') parts.push(meterOf(r.refresh.progress ?? ''))
  else if (r.refresh.state === 'starting') parts.push('starting…')
  const warnings = warningsOf(r.last_run).length + (r.vectors?.state === 'unreadable' ? 1 : 0)
  if (warnings > 0) parts.push(`⚠ ${warnings}`)
  if (r.tree.state === 'behind') parts.push(`${r.tree.files?.length ?? 0} behind`)
  if (r.tree.state === 'other_grammar') parts.push('re-read due')
  if (r.tree.state === 'newer_grammar') parts.push(`read-only, written by ${r.tree.release ? `zerograph ${r.tree.release}` : 'a newer release'}`)
  if (r.vectors?.state === 'rows' && r.vectors.owed > 0) parts.push(`${r.vectors.owed} to embed`)
  if (r.model.recorded && r.model.configured && r.model.recorded.toLowerCase() !== r.model.configured.toLowerCase()) parts.push('model switch due')
  if (parts.length === 1) parts.push('in sync')
  if (r.serve.running) parts.push('⇄ serve')
  return parts.join(' · ')
}

export type Run = { isRunning: boolean; meter?: string; last?: string; warnings: string[]; lines: string[] }

export function readRun(log: string, mtimeMs: number, now: number): Run | undefined {
  const all = log.split(/[\r\n]/).map(l => l.trim()).filter(l => l.length > 0)
  const started = all.lastIndexOf(RUN_MARKER)
  if (started < 0) return undefined
  const lines = all.slice(started + 1)
  return {
    isRunning: now - mtimeMs < QUIET_MS,
    meter: [...lines].reverse().find(isMeter),
    last: lines.at(-1),
    warnings: warningsOf(lines),
    lines,
  }
}

// `status` walks the whole tree to count the files behind, so the full report is taken rarely
// and the log and the socket, which cost a read and a stat, carry the state between.
export const FULL_MS = 5 * 60_000

// As many of the run's lines as `status` keeps in `last_run`.
const RUN_LINES = 10

export type Full = { report: Report; takenAt: number }
export type LiveLog = { mtimeMs: number; run: Run | undefined }
export type Live = { log?: LiveLog; socket?: { mtimeMs: number } }

// A zerograph that predates `serve.socket` binds its server inside the store.
export function socketOf(report: Report | null | undefined, store: string | undefined): string | undefined {
  return report?.serve.socket ?? (store === undefined ? undefined : `${store}/serve.sock`)
}

// Due when none was taken, when it is old, and once after a run logged since it ended, so the
// files behind and the vectors owed do not linger until the next interval.
export function isFullDue(takenAt: number | undefined, now: number, log: LiveLog | undefined): boolean {
  if (takenAt === undefined || now - takenAt >= FULL_MS) return true
  return log !== undefined && log.mtimeMs > takenAt && !log.run?.isRunning
}

// What changed since the full report, laid over it. A log untouched since then is the one the
// report already read, and the report also asked the writer lock, which the log's quiet cannot
// tell from a run that works without writing. A socket bound since then is a new server; an
// older one is the one the report asked, and only it could tell a live server from a file a
// killed one left.
export function overlay(full: Full, live: Live): Report {
  const { report, takenAt } = full
  const running = live.socket === undefined ? false : live.socket.mtimeMs > takenAt || report.serve.running
  const serve = { ...report.serve, running }
  if (!live.log || live.log.mtimeMs <= takenAt) return { ...report, serve }
  const { run } = live.log
  const refresh: Report['refresh'] = run?.isRunning ? { state: 'running', progress: run.meter ?? run.last ?? 'no output yet' } : { state: 'none' }
  const lastRun = (run?.lines ?? []).filter(l => !isMeter(l)).slice(-RUN_LINES)
  return { ...report, refresh, last_run: lastRun, serve }
}
