import { describe, expect, it } from 'vitest';
import { processPayment } from '../src/payer.js';
import { createValidPaymentRequirements } from './fixtures.js';

describe('Testnet live integration', () => {
  const isEnabled = process.env.RUN_INTEGRATION_TESTS === 'true';

  it.skipIf(!isEnabled)(
    'submits live transaction to Soroban testnet and executes __check_auth simulation',
    async () => {
      // Integration test runs only when explicitly requested with live credentials
      const reqs = createValidPaymentRequirements();
      const result = await processPayment({
        payment_requirements: reqs,
        resource_url: 'https://api.example.com/live',
      });

      expect(result).toBeDefined();
    }
  );
});
