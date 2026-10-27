#!/usr/bin/env node
import { processPayment } from './payer.js';
import { CliInput } from './types.js';

async function main() {
  let rawInput = '';
  process.stdin.setEncoding('utf8');

  for await (const chunk of process.stdin) {
    rawInput += chunk;
  }

  let input: CliInput;
  try {
    input = JSON.parse(rawInput);
  } catch (err) {
    const errorOutput = {
      ok: false,
      code: 'INVALID_CLI_INPUT',
      message: `Invalid JSON received on stdin: ${err instanceof Error ? err.message : String(err)}`,
    };
    process.stdout.write(JSON.stringify(errorOutput) + '\n');
    process.exit(1);
  }

  const result = await processPayment(input);
  process.stdout.write(JSON.stringify(result) + '\n');

  if (!result.ok) {
    process.exit(1);
  }
}

main().catch((err) => {
  // Guarantee strict JSON on stdout even on unexpected errors
  const fallbackError = {
    ok: false,
    code: 'UNEXPECTED_ERROR',
    message: err instanceof Error ? err.message : 'Unexpected payer error',
  };
  process.stderr.write(`[payer error] ${err instanceof Error ? err.stack : String(err)}\n`);
  process.stdout.write(JSON.stringify(fallbackError) + '\n');
  process.exit(1);
});
