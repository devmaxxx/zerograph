import type { On } from 'claude-code'
import { expect, test } from 'claude-code/testing'

const ROOT = '/work/cloned'
const MACHINE = '/usr/local/bin'

// A checkout that ships a zerograph everywhere a local install or a build would put one, a PATH
// that names it relatively and absolutely before the machine's, and no binary that answers, so
// whatever the mod resolves is asked.
function cloned(on: On, path: string): string[][] {
  const ran: string[][] = []
  on('session.root', async () => ({ value: ROOT }))
  on('env.get', async ($, e) => ({ value: e.name === 'PATH' ? path : undefined }))
  on('fs.exists', async () => ({ value: true }))
  on('fs.stat', async ($, e) => {
    const shipped = e.path.startsWith(`${ROOT}/`) || e.path.startsWith('node_modules/') || e.path === `${MACHINE}/zerograph`
    if (!shipped) throw new Error('ENOENT')
    return { value: { kind: 'file', size: 1, mtimeMs: 0 } }
  })
  on('process.run', async ($, e) => {
    ran.push([...e.argv])
    return { value: { exitCode: 1, stdout: '', stderr: '', isStdoutTruncated: false } }
  })
  return ran
}

const SHADOWED = ['node_modules/.bin', '.', `${ROOT}/node_modules/.bin`, MACHINE].join(':')

test('with no binary configured, the zerograph on PATH is asked by its absolute path, never one under the checkout', async ($, on) => {
  const ran = cloned(on, SHADOWED)
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv[0] !== `${MACHINE}/zerograph`)).toEqual([])
})

test('with only the checkout on PATH, nothing is run', async ($, on) => {
  const ran = cloned(on, ['node_modules/.bin', `${ROOT}/node_modules/.bin`].join(':'))
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran).toEqual([])
})

test('a configured binary is the one asked', { options: { binary: '/opt/zerograph/bin/zerograph' } }, async ($, on) => {
  const ran = cloned(on, SHADOWED)
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv[0] !== '/opt/zerograph/bin/zerograph')).toEqual([])
})

test('a relative configured binary names the checkout\'s file, so nothing is run', { options: { binary: 'node_modules/.bin/zerograph' } }, async ($, on) => {
  const ran = cloned(on, SHADOWED)
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran).toEqual([])
})
