import { expect, test } from 'claude-code/testing'

import { FULL_MS, QUIET_MS, isFullDue, lineOf, overlay, readRun, socketOf, type Report } from './parse'

const base: Report = {
  index: { state: 'built', doc_nodes: 10, code_nodes: 25, edges: 183 },
  tree: { state: 'in_step' },
  model: { recorded: 'm', configured: 'M' },
  vectors: { state: 'rows', rows: 35, owed: 0 },
  refresh: { state: 'none' },
  last_run: [],
  serve: { running: false, up_secs: null, log: [] },
}

test('a store in step and idle reads as in sync', () => {
  expect(lineOf(base)).toBe('◇ zerograph · in sync')
})

test('a running refresh shows a bar first, then the warnings and what is behind', () => {
  const r: Report = {
    ...base,
    tree: { state: 'behind', files: ['a', 'b', 'c'] },
    refresh: { state: 'running', progress: 'embedded 32/40 (8.0 rows/s, ~1 s left)' },
    last_run: ['refresh: 3 changed, 0 removed', 'dense: index unreadable, continuing lexical-only (bad header)'],
    serve: { running: true, up_secs: 60, log: [] },
  }
  expect(lineOf(r)).toBe('◆ zerograph · ▰▰▰▰▰▰▱▱ 80% · 1s left · ⚠ 1 · 3 behind · ⇄ serve')
})

test('owed vectors and a configured model the vectors are not under are both due', () => {
  const r: Report = { ...base, vectors: { state: 'rows', rows: 35, owed: 4 }, model: { recorded: 'a', configured: 'b' } }
  expect(lineOf(r)).toBe('◇ zerograph · 4 to embed · model switch due')
})

test('a store a newer release wrote reads as read-only and names that release', () => {
  expect(lineOf({ ...base, tree: { state: 'newer_grammar', grammar: 7, release: '0.7.0' } })).toBe('◇ zerograph · read-only, written by zerograph 0.7.0')
  expect(lineOf({ ...base, tree: { state: 'newer_grammar', grammar: 7, release: '' } })).toBe('◇ zerograph · read-only, written by a newer release')
})

test('an empty index says so', () => {
  expect(lineOf({ ...base, index: { state: 'empty' } })).toBe('◇ zerograph · no index')
  expect(lineOf({ ...base, index: { state: 'unreadable', error: 'x' } })).toBe('◇ zerograph · ⚠ index unreadable')
})

const LOG = ['embedded 99/100 (1.0 rows/s, ~1 s left)', 'refresh: started', 'dense: index unreadable (x)', 'embedded 16/40 (8.0 rows/s, ~3 s left)'].join('\n')

test('the log fallback reads only the run after the last marker', () => {
  const run = readRun(LOG, 1000, 2000)
  expect(run?.warnings).toEqual(['dense: index unreadable (x)'])
  expect(run?.isRunning).toBe(true)
  expect(run?.meter).toBe('embedded 16/40 (8.0 rows/s, ~3 s left)')
})

test('the log fallback treats a quiet log as a run that ended', () => {
  expect(readRun(LOG, 0, QUIET_MS + 1)?.isRunning).toBe(false)
  expect(readRun('embedded 1/2\n', 1000, 2000)).toBeUndefined()
})

const TAKEN = 1_000_000
const full: Report = {
  ...base,
  tree: { state: 'behind', files: ['a'] },
  vectors: { state: 'rows', rows: 35, owed: 4 },
  refresh: { state: 'none' },
  last_run: ['refresh: 1 changed, 0 removed'],
  serve: { running: false, socket: '/home/u/.cache/zerograph/sock/1a2b.sock', up_secs: null, log: [] },
}
const RUNNING = ['refresh: started', 'refresh: 2 changed, 0 removed', 'embedded 8/40 (8.0 rows/s, ~4 s left)'].join('\n')

test('a log written since the full report replaces its refresh and last run, and the rest stands', () => {
  const now = TAKEN + 5_000
  const log = { mtimeMs: TAKEN + 4_000, run: readRun(RUNNING, TAKEN + 4_000, now) }
  const r = overlay({ report: full, takenAt: TAKEN }, { log, socket: { mtimeMs: TAKEN + 1_000 } })
  expect(r.refresh).toEqual({ state: 'running', progress: 'embedded 8/40 (8.0 rows/s, ~4 s left)' })
  expect(r.last_run).toEqual(['refresh: 2 changed, 0 removed'])
  expect(r.serve.running).toBe(true)
  expect(r.index).toEqual(full.index)
  expect(r.tree).toEqual(full.tree)
  expect(r.vectors).toEqual(full.vectors)
  expect(r.model).toEqual(full.model)
})

test('a run that went quiet since the full report reads as none running, its lines kept', () => {
  const ended = ['refresh: started', ...Array.from({ length: 12 }, (_, i) => `line ${i}`)].join('\n')
  const log = { mtimeMs: TAKEN + 1, run: readRun(ended, TAKEN + 1, TAKEN + QUIET_MS + 2) }
  const r = overlay({ report: { ...full, refresh: { state: 'running', progress: 'x' } }, takenAt: TAKEN }, { log })
  expect(r.refresh).toEqual({ state: 'none' })
  expect(r.last_run).toEqual(Array.from({ length: 10 }, (_, i) => `line ${i + 2}`))
})

test('a log not written since the full report leaves the report its word on the run', () => {
  const report: Report = { ...full, refresh: { state: 'running', progress: 'walking' } }
  const log = { mtimeMs: TAKEN - 1, run: readRun(RUNNING, TAKEN - 1, TAKEN + QUIET_MS * 2) }
  const r = overlay({ report, takenAt: TAKEN }, { log, socket: undefined })
  expect(r.refresh).toEqual(report.refresh)
  expect(r.last_run).toEqual(report.last_run)
})

test('the socket reads as serving when present, unless it is the one the full report found dead', () => {
  const at = (running: boolean) => ({ report: { ...full, serve: { ...full.serve, running } }, takenAt: TAKEN })
  expect(overlay(at(true), {}).serve.running).toBe(false)
  expect(overlay(at(false), { socket: { mtimeMs: TAKEN + 1 } }).serve.running).toBe(true)
  expect(overlay(at(false), { socket: { mtimeMs: TAKEN - 1 } }).serve.running).toBe(false)
  expect(overlay(at(true), { socket: { mtimeMs: TAKEN - 1 } }).serve.running).toBe(true)
})

test('the socket checked is the one the full report named, the store one only for a report without it', () => {
  expect(socketOf(full, '/repo/.zerograph')).toBe('/home/u/.cache/zerograph/sock/1a2b.sock')
  const older: Report = { ...full, serve: { running: false, up_secs: null, log: [] } }
  expect(socketOf(older, '/repo/.zerograph')).toBe('/repo/.zerograph/serve.sock')
})

test('a full check is due with none taken yet and once five minutes have passed', () => {
  expect(isFullDue(undefined, TAKEN, undefined)).toBe(true)
  expect(isFullDue(TAKEN, TAKEN + FULL_MS - 1, undefined)).toBe(false)
  expect(isFullDue(TAKEN, TAKEN + FULL_MS, undefined)).toBe(true)
  expect(FULL_MS).toBe(5 * 60_000)
})

test('a full check is due once a run logged since the last one has ended, and not while it runs', () => {
  const now = TAKEN + 2 * QUIET_MS
  const quiet = { mtimeMs: TAKEN + 1, run: readRun(RUNNING, TAKEN + 1, now) }
  expect(isFullDue(TAKEN, now, quiet)).toBe(true)
  const live = { mtimeMs: now - 1_000, run: readRun(RUNNING, now - 1_000, now) }
  expect(isFullDue(TAKEN, now, live)).toBe(false)
  const before = { mtimeMs: TAKEN - 1, run: readRun(RUNNING, TAKEN - 1, now) }
  expect(isFullDue(TAKEN, now, before)).toBe(false)
})

test('a newer release the CLI cached ends the line with its marker, and the live overlay keeps it', () => {
  const r: Report = { ...base, update: '0.7.1' }
  expect(lineOf(r)).toBe('◇ zerograph · in sync · ↑0.7.1')
  expect(lineOf({ ...base, update: null })).toBe('◇ zerograph · in sync')
  expect(lineOf(overlay({ report: r, takenAt: 0 }, { log: { mtimeMs: 1, run: undefined } }))).toBe('◇ zerograph · in sync · ↑0.7.1')
})
