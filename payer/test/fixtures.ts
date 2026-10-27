import { Address, Keypair } from '@stellar/stellar-sdk';
import { PaymentRequirements, PayerConfig } from '../src/types.js';

export const TEST_AGENT_KEYPAIR = Keypair.random();

export const TEST_ACCOUNT_ID = Address.contract(Buffer.alloc(32, 1)).toString();
export const TEST_ASSET_ID = Address.contract(Buffer.alloc(32, 2)).toString();
export const TEST_PAYEE_ID = Keypair.random().publicKey();

export const TEST_CONFIG: PayerConfig = {
  accountContractId: TEST_ACCOUNT_ID,
  agentSecretKey: TEST_AGENT_KEYPAIR.secret(),
  sorobanRpcUrl: 'https://soroban-testnet.stellar.org',
  network: 'stellar:testnet',
  networkPassphrase: 'Test SDF Network ; September 2015',
  maxPerCall: 10_000_000n,
  expirationSeconds: 60,
};

export function createValidPaymentRequirements(
  overrides?: Partial<PaymentRequirements>
): PaymentRequirements {
  return {
    scheme: 'exact',
    network: 'stellar:testnet',
    asset: TEST_ASSET_ID,
    payTo: TEST_PAYEE_ID,
    amount: '1000000',
    extra: {
      areFeesSponsored: true,
    },
    maxTimeoutSeconds: 60,
    ...overrides,
  };
}
