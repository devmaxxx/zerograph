import { expect, test } from 'claude-code/testing'

import { QUIET_MS, lineOf, readRun, type Report } from './parse'

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
