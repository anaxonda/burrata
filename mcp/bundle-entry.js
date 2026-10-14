import { main } from './server.js';

void main().catch(() => {
  console.error('Burrata MCP server could not start.');
  process.exitCode = 1;
});
