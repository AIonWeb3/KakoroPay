import { describe, expect, it } from 'vitest';
import { validatePreSignGuards } from '../src/guards.js';
import { PayerError } from '../src/types.js';
import {
  createValidPaymentRequirements,
  TEST_CONFIG,
} from './fixtures.js';

describe('Pre-sign guards', () => {
  it('rejects missing or non-HTTPS resource URL', async () => {
    const reqs = createValidPaymentRequirements();

    await expect(
      validatePreSignGuards({
        requirements: reqs,
        resourceUrl: 'http://api.example.com/data',
        config: TEST_CONFIG,
        queryPayeeRule: async () => 1,
      })
    ).rejects.toThrowError(
      expect.objectContaining({
        code: 'NON_HTTPS_RESOURCE_URL',
      })
    );

    await expect(
      validatePreSignGuards({
        requirements: reqs,
        resourceUrl: '',
        config: TEST_CONFIG,
        queryPayeeRule: async () => 1,
      })
    ).rejects.toThrowError(
      expect.objectContaining({
        code: 'NON_HTTPS_RESOURCE_URL',
      })
    );
  });

  it('rejects network mismatch with config', async () => {
    const reqs = createValidPaymentRequirements({
      network: 'stellar:pubnet',
    });

    await expect(
      validatePreSignGuards({
        requirements: reqs,
        resourceUrl: 'https://api.example.com/data',
        config: TEST_CONFIG, // configured with stellar:testnet
        queryPayeeRule: async () => 1,
      })
    ).rejects.toThrowError(
      expect.objectContaining({
        code: 'NETWORK_MISMATCH',
      })
    );
  });

  it('rejects missing, non-integer, or non-positive amount', async () => {
    const invalidAmounts = ['', '0', '-500', '12.34', 'abc'];

    for (const amount of invalidAmounts) {
      const reqs = createValidPaymentRequirements({ amount });
      await expect(
        validatePreSignGuards({
          requirements: reqs,
          resourceUrl: 'https://api.example.com/data',
          config: TEST_CONFIG,
          queryPayeeRule: async () => 1,
        })
      ).rejects.toThrowError(
        expect.objectContaining({
          code: 'INVALID_AMOUNT',
        })
      );
    }
  });

  it('rejects amount above PAYER_MAX_PER_CALL', async () => {
    const reqs = createValidPaymentRequirements({
      amount: '10000001', // maxPerCall is 10_000_000
    });

    await expect(
      validatePreSignGuards({
        requirements: reqs,
        resourceUrl: 'https://api.example.com/data',
        config: TEST_CONFIG,
        queryPayeeRule: async () => 1,
      })
    ).rejects.toThrowError(
      expect.objectContaining({
        code: 'AMOUNT_EXCEEDS_MAX_PER_CALL',
      })
    );
  });

  it('rejects asset or pay_to not present in on-chain policy', async () => {
    const reqs = createValidPaymentRequirements();

    await expect(
      validatePreSignGuards({
        requirements: reqs,
        resourceUrl: 'https://api.example.com/data',
        config: TEST_CONFIG,
        queryPayeeRule: async () => null, // not found
      })
    ).rejects.toThrowError(
      expect.objectContaining({
        code: 'PAYEE_NOT_ALLOWLISTED',
      })
    );
  });

  it('accepts valid requirements and returns bound rule ID', async () => {
    const reqs = createValidPaymentRequirements({
      amount: '5000000',
    });

    const result = await validatePreSignGuards({
      requirements: reqs,
      resourceUrl: 'https://api.example.com/data',
      config: TEST_CONFIG,
      queryPayeeRule: async () => 42,
    });

    expect(result.ruleId).toBe(42);
  });
});
