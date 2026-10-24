import { execFileSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';

// npm runs scripts in cmd.exe on Windows, where `mkdir -p` and node_modules/.bin's shell shims
// do not work, and protoc needs an absolute path to run a .cmd plugin.
const plugin = resolve('node_modules/.bin', process.platform === 'win32' ? 'protoc-gen-es.cmd' : 'protoc-gen-es');
mkdirSync('gen', { recursive: true });
execFileSync(
  'protoc',
  ['-I', '../protocol', `--plugin=protoc-gen-es=${plugin}`, '--es_out=gen', '--es_opt=target=ts', '../protocol/gem.proto'],
  { stdio: 'inherit' },
);
