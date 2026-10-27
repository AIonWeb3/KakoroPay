import { describe, expect, it, vi } from 'vitest';
import { processPayment } from '../src/payer.js';
import {
  createValidPaymentRequirements,
  TEST_AGENT_KEYPAIR,
  TEST_CONFIG,
} from './fixtures.js';

describe('Payer Worker CLI and Payment Processing', () => {
  it('returns ok: true with authorized payment payload for valid allowlisted requirement', async () => {
    const reqs = createValidPaymentRequirements();

    const mockSchemeClient = {
      createPaymentPayload: vi.fn().mockResolvedValue({
        x402Version: 1,
        payload: {
          transaction: 'AAAA_MOCK_SIGNED_TX_XDR_BASE64',
        },
      }),
    };

    const result = await processPayment(
      {
        payment_requirements: reqs,
        resource_url: 'https://api.example.com/v1/resource',
      },
      {
        config: TEST_CONFIG,
        queryPayeeRule: async () => 3,
        schemeClient: mockSchemeClient,
      }
    );

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.payment_payload.transaction).toBe('AAAA_MOCK_SIGNED_TX_XDR_BASE64');
      expect(result.rule_id).toBe(3);
    }
    expect(mockSchemeClient.createPaymentPayload).toHaveBeenCalledOnce();
  });

  it('returns ok: false with PAYEE_NOT_ALLOWLISTED when payee is not allowlisted', async () => {
    const reqs = createValidPaymentRequirements();
    const mockSchemeClient = {
      createPaymentPayload: vi.fn(),
    };

    const result = await processPayment(
      {
        payment_requirements: reqs,
        resource_url: 'https://api.example.com/v1/resource',
      },
      {
        config: TEST_CONFIG,
        queryPayeeRule: async () => null, // Not allowlisted
        schemeClient: mockSchemeClient,
      }
    );

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.code).toBe('PAYEE_NOT_ALLOWLISTED');
      expect(result.message).toContain('not allowlisted');
    }
    // Never called scheme or produced signature
    expect(mockSchemeClient.createPaymentPayload).not.toHaveBeenCalled();
  });

  it('returns ok: false with AMOUNT_EXCEEDS_MAX_PER_CALL when amount exceeds ceiling', async () => {
    const reqs = createValidPaymentRequirements({
      amount: '50000000', // exceeds maxPerCall 10_000_000
    });
    const mockSchemeClient = {
      createPaymentPayload: vi.fn(),
    };

    const result = await processPayment(
      {
        payment_requirements: reqs,
        resource_url: 'https://api.example.com/v1/resource',
      },
      {
        config: TEST_CONFIG,
        queryPayeeRule: async () => 1,
        schemeClient: mockSchemeClient,
      }
    );

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.code).toBe('AMOUNT_EXCEEDS_MAX_PER_CALL');
    }
    expect(mockSchemeClient.createPaymentPayload).not.toHaveBeenCalled();
  });

  it('returns ok: false with NETWORK_MISMATCH when network does not match config', async () => {
    const reqs = createValidPaymentRequirements({
      network: 'stellar:pubnet',
    });
    const mockSchemeClient = {
      createPaymentPayload: vi.fn(),
    };

    const result = await processPayment(
      {
        payment_requirements: reqs,
        resource_url: 'https://api.example.com/v1/resource',
      },
      {
        config: TEST_CONFIG,
        queryPayeeRule: async () => 1,
        schemeClient: mockSchemeClient,
      }
    );

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.code).toBe('NETWORK_MISMATCH');
    }
    expect(mockSchemeClient.createPaymentPayload).not.toHaveBeenCalled();
  });

  it('maps contract simulation errors to exact error names', async () => {
    const contractErrors = [
      { msg: 'HostError: Error(Contract, #5)', expectedCode: 'OverBudget' },
      { msg: 'Simulation failed: RateLimited', expectedCode: 'RateLimited' },
      { msg: 'Error(Contract, #7)', expectedCode: 'InvalidContext' },
      { msg: 'HostError: AgentGuardExecutionDenied', expectedCode: 'AgentGuardExecutionDenied' },
    ];

    for (const { msg, expectedCode } of contractErrors) {
      const reqs = createValidPaymentRequirements();
      const mockSchemeClient = {
        createPaymentPayload: vi.fn().mockRejectedValue(new Error(msg)),
      };

      const result = await processPayment(
        {
          payment_requirements: reqs,
          resource_url: 'https://api.example.com/v1/resource',
        },
        {
          config: TEST_CONFIG,
          queryPayeeRule: async () => 1,
          schemeClient: mockSchemeClient,
        }
      );

      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.code).toBe(expectedCode);
      }
    }
  });

  it('ensures secret key is never leaked in errors or messages', async () => {
    const secret = TEST_AGENT_KEYPAIR.secret();

    const result = await processPayment(
      {
        payment_requirements: createValidPaymentRequirements(),
        resource_url: 'http://insecure.example.com',
      },
      {
        config: TEST_CONFIG,
        queryPayeeRule: async () => 1,
      }
    );

    const json = JSON.stringify(result);
    expect(json).not.toContain(secret);
    if (!result.ok) {
      expect(result.message).not.toContain(secret);
    }
  });
});
