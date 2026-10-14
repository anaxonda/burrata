import { basename } from 'node:path';
import { createServer, main } from './server.js';

export { createServer, main };

if (process.argv[1] && basename(process.argv[1]) === 'server.cjs') {
  void main().catch(() => {
    console.error('Burrata MCP server could not start.');
    process.exitCode = 1;
  });
}
