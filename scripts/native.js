// Native app launcher (used by F5): builds and runs the Rust app via `cargo run -p mangavox`.
const { spawn, execSync } = require('child_process')
const path = require('path')

delete process.env.NODE_OPTIONS // js-debug bootloader; not meant for cargo or the app
const child = spawn('cargo', ['run', '-p', 'mangavox', ...process.argv.slice(2)], {
  cwd: path.join(__dirname, '..', 'native'),
  stdio: 'inherit',
  env: process.env
})
const stop = () => {
  try { execSync(`taskkill /pid ${child.pid} /T /F`, { stdio: 'ignore' }) } catch {}
  process.exit(0)
}
process.on('SIGINT', stop)
process.on('SIGTERM', stop)
child.on('exit', (code) => process.exit(code ?? 0))
