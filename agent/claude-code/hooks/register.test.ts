import type { On } from 'claude-code'
import { expect, test } from 'claude-code/testing'

const ROOT = '/work/cloned'

// A checkout that ships a zerograph everywhere a local install or a build would put one: every
// path exists, and no binary answers, so each candidate the mod would try is asked in turn.
function cloned(on: On): string[][] {
  const ran: string[][] = []
  on('session.root', async () => ({ value: ROOT }))
  on('fs.exists', async () => ({ value: true }))
  on('process.run', async ($, e) => {
    ran.push([...e.argv])
    return { value: { exitCode: 1, stdout: '', stderr: '', isStdoutTruncated: false } }
  })
  return ran
}

test('with no binary configured, only the zerograph on PATH is asked, never one under the checkout', async ($, on) => {
  const ran = cloned(on)
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv[0] !== 'zerograph')).toEqual([])
})

test('a configured binary is the one asked', { options: { binary: '/opt/zerograph/bin/zerograph' } }, async ($, on) => {
  const ran = cloned(on)
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv[0] !== '/opt/zerograph/bin/zerograph')).toEqual([])
})
