const test = require('node:test');
const assert = require('node:assert/strict');
const { finishTerminal } = require('./pty-host.cjs');

test('finishes the host after writing the PTY exit event', () => {
  let exitedWith;
  let written;
  const terminal = {};
  const output = {
    write(value, callback) {
      written = value;
      callback();
    },
  };

  finishTerminal(terminal, { exitCode: 0, signal: 0 }, output, (code) => {
    exitedWith = code;
  });

  assert.equal(written, '{"type":"exit","code":0,"signal":0}\n');
  assert.equal(exitedWith, 0);
});

test('reports an unknown PTY exit code as a failure', () => {
  let exitedWith;
  const output = {
    write(_value, callback) {
      callback();
    },
  };

  finishTerminal({}, { signal: 1 }, output, (code) => {
    exitedWith = code;
  });

  assert.equal(exitedWith, 1);
});
