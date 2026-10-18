// Cross-platform dev launcher: clears ELECTRON_RUN_AS_NODE (set by VS Code) then runs electron-vite dev.
const { spawn } = require('child_process')
const path = require('path')

delete process.env.ELECTRON_RUN_AS_NODE
const bin = path.join(__dirname, '..', 'node_modules', 'electron-vite', 'bin', 'electron-vite.js')
const child = spawn(process.execPath, [bin, 'dev', ...process.argv.slice(2)], { stdio: 'inherit', env: process.env })
child.on('exit', (code) => process.exit(code ?? 0))
