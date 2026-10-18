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

test('a bare configured name is a PATH lookup, as it was before absolute paths were required', { options: { binary: 'zerograph' } }, async ($, on) => {
  const ran = cloned(on, SHADOWED)
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv[0] !== `${MACHINE}/zerograph`)).toEqual([])
})

// A Windows machine: `files` are the paths that exist, matched as Windows matches them. The engine
// under test resolves a drive path against this machine's working directory, so a match is a suffix.
function windows(on: On, root: string, path: string, files: string[], checkout = root): string[][] {
  const ran: string[][] = []
  const at = (path: string, file: string) => path.toLowerCase().endsWith(file.toLowerCase())
  on('session.root', async () => ({ value: root }))
  on('env.get', async ($, e) => ({ value: { PATH: path, ComSpec: 'C:\\Windows\\system32\\cmd.exe' }[e.name] }))
  on('fs.exists', async ($, e) => ({ value: at(e.path, `${checkout}/.git`) }))
  on('fs.stat', async ($, e) => {
    if (!files.some(f => at(e.path, f))) throw new Error('ENOENT')
    return { value: { kind: 'file', size: 1, mtimeMs: 0 } }
  })
  on('process.run', async ($, e) => {
    ran.push([...e.argv])
    return { value: { exitCode: 1, stdout: '', stderr: '', isStdoutTruncated: false } }
  })
  return ran
}

test('on Windows a zerograph.exe anywhere on PATH is asked before an npm .cmd shim', async ($, on) => {
  const ran = windows(on, 'C:\\work\\cloned', 'C:\\npm;C:\\cargo\\bin', ['C:\\npm\\zerograph.cmd', 'C:\\cargo\\bin\\zerograph.exe'])
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv[0] !== 'C:\\cargo\\bin\\zerograph.exe')).toEqual([])
})

test('on Windows an npm .cmd shim is run through cmd, which alone can start one', async ($, on) => {
  const ran = windows(on, 'C:\\work\\cloned', 'C:\\Users\\Max Smith\\AppData\\Roaming\\npm', ['C:\\Users\\Max Smith\\AppData\\Roaming\\npm\\zerograph.cmd'])
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran.length).toBeGreaterThan(0)
  expect(ran.filter(argv => argv.slice(0, 5).join('|') !== 'C:\\Windows\\system32\\cmd.exe|/d|/c|call|C:\\Users\\Max Smith\\AppData\\Roaming\\npm\\zerograph.cmd')).toEqual([])
})

test('on Windows a PATH entry inside the checkout is the checkout\'s however its case is spelled', async ($, on) => {
  const ran = windows(on, 'C:\\Work\\Cloned', 'c:\\work\\cloned\\node_modules\\.bin', ['C:\\Work\\Cloned\\node_modules\\.bin\\zerograph.exe'])
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran).toEqual([])
})

test('a session opened in a package of a checkout still refuses a PATH entry elsewhere in that checkout', async ($, on) => {
  const ran = windows(on, 'C:\\work\\cloned\\packages\\x', 'C:\\work\\cloned\\node_modules\\.bin', ['C:\\work\\cloned\\node_modules\\.bin\\zerograph.exe'], 'C:\\work\\cloned')
  await $.command.run({ command: 'zerograph', args: '' })
  expect(ran).toEqual([])
})
