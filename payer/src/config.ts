import fs from 'node:fs';
import { Keypair } from '@stellar/stellar-sdk';
import { PayerConfig, PayerError } from './types.js';

export function loadConfig(overrides?: Partial<PayerConfig>): PayerConfig {
  const accountContractId =
    overrides?.accountContractId ||
    process.env.ACCOUNT_CONTRACT_ID ||
    '';

  let agentSecretKey = overrides?.agentSecretKey || '';
  if (!agentSecretKey) {
    if (process.env.AGENT_SIGNER_SECRET) {
      agentSecretKey = process.env.AGENT_SIGNER_SECRET.trim();
    } else if (process.env.AGENT_SIGNER_SECRET_PATH) {
      const secretPath = process.env.AGENT_SIGNER_SECRET_PATH.trim();
      try {
        agentSecretKey = fs.readFileSync(secretPath, 'utf8').trim();
      } catch {
        throw new PayerError(
          'MISSING_CONFIG',
          'Could not read agent secret from AGENT_SIGNER_SECRET_PATH'
        );
      }
    }
  }

  if (!agentSecretKey) {
    throw new PayerError(
      'MISSING_CONFIG',
      'AGENT_SIGNER_SECRET or AGENT_SIGNER_SECRET_PATH must be provided'
    );
  }

  // Validate secret key format safely without exposing it
  try {
    Keypair.fromSecret(agentSecretKey);
  } catch {
    throw new PayerError(
      'MISSING_CONFIG',
      'Invalid agent secret key format (must be a valid Stellar secret seed)'
    );
  }

  if (!accountContractId) {
    throw new PayerError(
      'MISSING_CONFIG',
      'ACCOUNT_CONTRACT_ID must be configured with the agent account C... contract address'
    );
  }

  const sorobanRpcUrl =
    overrides?.sorobanRpcUrl ||
    process.env.SOROBAN_RPC_URL ||
    'https://soroban-testnet.stellar.org';

  const network =
    overrides?.network ||
    process.env.STELLAR_NETWORK ||
    'stellar:testnet';

  const networkPassphrase =
    overrides?.networkPassphrase ||
    process.env.STELLAR_NETWORK_PASSPHRASE ||
    process.env.NETWORK_PASSPHRASE ||
    'Test SDF Network ; September 2015';

  const maxPerCallRaw =
    overrides?.maxPerCall?.toString() ||
    process.env.PAYER_MAX_PER_CALL ||
    '10000000';

  let maxPerCall: bigint;
  try {
    maxPerCall = BigInt(maxPerCallRaw);
    if (maxPerCall <= 0n) throw new Error();
  } catch {
    throw new PayerError(
      'MISSING_CONFIG',
      'PAYER_MAX_PER_CALL must be a positive integer'
    );
  }

  const expirationSecondsRaw =
    overrides?.expirationSeconds?.toString() ||
    process.env.PAYER_EXPIRATION_SECONDS ||
    '60';

  const expirationSeconds = Number.parseInt(expirationSecondsRaw, 10);
  if (Number.isNaN(expirationSeconds) || expirationSeconds <= 0) {
    throw new PayerError(
      'MISSING_CONFIG',
      'PAYER_EXPIRATION_SECONDS must be a positive integer'
    );
  }

  return {
    accountContractId,
    agentSecretKey,
    sorobanRpcUrl,
    network,
    networkPassphrase,
    maxPerCall,
    expirationSeconds,
  };
}
