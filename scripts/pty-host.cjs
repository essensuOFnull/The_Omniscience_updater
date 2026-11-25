const path = require('node:path');

const actions = {
  winget: 'Install-Winget.ps1',
  store: 'Install-Store.ps1',
};

function finishTerminal(terminal, exitInfo, output = process.stdout, exit = process.exit) {
  const code = Number.isInteger(exitInfo.exitCode) ? exitInfo.exitCode : 1;
  output.write(`${JSON.stringify({ type: 'exit', code, signal: exitInfo.signal })}\n`, () => {
    exit(code);
  });
}

function runInstaller(action) {
  const scriptName = actions[action];
  if (!scriptName) {
    process.stderr.write('Unsupported installer action.\n');
    process.exit(2);
    return;
  }

  try {
    const { spawn } = require('@lydell/node-pty');
    const baseDirectory = path.basename(__dirname).toLowerCase() === 'scripts'
      ? path.dirname(__dirname)
      : __dirname;
    const scriptPath = path.join(baseDirectory, 'scripts', scriptName);
    const shell = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
    const terminal = spawn(shell, [
      '-NoLogo',
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      scriptPath,
    ], {
      name: 'xterm-256color',
      cols: 120,
      rows: 36,
      cwd: baseDirectory,
      env: { ...process.env, TERM: 'xterm-256color' },
    });

    terminal.onData((data) => {
      process.stdout.write(`${JSON.stringify({ type: 'output', data })}\n`);
    });
    terminal.onExit((exitInfo) => finishTerminal(terminal, exitInfo));
  } catch (error) {
    process.stderr.write(`Could not start PowerShell PTY: ${error.message}\n`);
    process.exit(1);
  }
}

module.exports = { finishTerminal };

if (require.main === module) runInstaller(process.argv[2]);
