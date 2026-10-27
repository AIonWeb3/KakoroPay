import {
  Account,
  Address,
  Contract,
  nativeToScVal,
  rpc,
  scValToNative,
  TransactionBuilder,
} from '@stellar/stellar-sdk';
import { PaymentRequirements, PayerConfig, PayerError } from './types.js';

const DUMMY_ACCOUNT_ID = 'GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF';

export function normalizeNetwork(net: string): string {
  const lower = net.trim().toLowerCase();
  if (lower === 'testnet' || lower === 'stellar:testnet') return 'stellar:testnet';
  if (lower === 'public' || lower === 'pubnet' || lower === 'mainnet' || lower === 'stellar:pubnet') {
    return 'stellar:pubnet';
  }
  return lower;
}

export async function queryPayeeRuleOnChain(
  config: PayerConfig,
  asset: string,
  payTo: string
): Promise<number | null> {
  const server = new rpc.Server(config.sorobanRpcUrl);
  const contract = new Contract(config.accountContractId);

  const tx = new TransactionBuilder(new Account(DUMMY_ACCOUNT_ID, '0'), {
    fee: '100',
    networkPassphrase: config.networkPassphrase,
  })
    .addOperation(
      contract.call(
        'get_payee_rule',
        nativeToScVal(asset, { type: 'address' }),
        nativeToScVal(payTo, { type: 'address' })
      )
    )
    .setTimeout(30)
    .build();

  const sim = await server.simulateTransaction(tx);
  if (!rpc.Api.isSimulationSuccess(sim) || !sim.result?.retval) {
    return null;
  }

  const native = scValToNative(sim.result.retval);
  if (native === null || native === undefined) {
    return null;
  }
  if (Array.isArray(native)) {
    return native.length > 0 ? Number(native[0]) : null;
  }
  if (typeof native === 'number' || typeof native === 'bigint') {
    return Number(native);
  }
  return null;
}

export interface ValidateGuardsOptions {
  requirements: PaymentRequirements;
  resourceUrl?: string;
  config: PayerConfig;
  queryPayeeRule?: (asset: string, payTo: string) => Promise<number | null>;
}

export async function validatePreSignGuards(
  options: ValidateGuardsOptions
): Promise<{ ruleId: number }> {
  const { requirements, resourceUrl, config, queryPayeeRule } = options;

  // 1. Requirement from a non-HTTPS resource URL
  if (!resourceUrl || typeof resourceUrl !== 'string') {
    throw new PayerError(
      'NON_HTTPS_RESOURCE_URL',
      'Resource URL must be provided and must use HTTPS protocol'
    );
  }

  const trimmedUrl = resourceUrl.trim();
  if (!trimmedUrl.toLowerCase().startsWith('https://')) {
    throw new PayerError(
      'NON_HTTPS_RESOURCE_URL',
      `Insecure resource URL: "${trimmedUrl}". Payment requirements must originate from an HTTPS URL.`
    );
  }

  // 2. Network mismatch with config
  if (!requirements.network || typeof requirements.network !== 'string') {
    throw new PayerError('NETWORK_MISMATCH', 'Missing payment requirement network');
  }

  const reqNetwork = normalizeNetwork(requirements.network);
  const configNetwork = normalizeNetwork(config.network);
  if (reqNetwork !== configNetwork) {
    throw new PayerError(
      'NETWORK_MISMATCH',
      `Payment network "${requirements.network}" does not match configured network "${config.network}"`
    );
  }

  // 3. Amount missing or non-positive
  if (
    requirements.amount === undefined ||
    requirements.amount === null ||
    typeof requirements.amount !== 'string' ||
    requirements.amount.trim() === ''
  ) {
    throw new PayerError(
      'INVALID_AMOUNT',
      'Payment amount is required and must be a string'
    );
  }

  let amountBigInt: bigint;
  try {
    const trimmedAmount = requirements.amount.trim();
    if (!/^\d+$/.test(trimmedAmount)) {
      throw new Error('Not an integer string');
    }
    amountBigInt = BigInt(trimmedAmount);
  } catch {
    throw new PayerError(
      'INVALID_AMOUNT',
      `Payment amount "${requirements.amount}" must be a positive integer`
    );
  }

  if (amountBigInt <= 0n) {
    throw new PayerError(
      'INVALID_AMOUNT',
      `Payment amount "${requirements.amount}" must be positive (> 0)`
    );
  }

  // 4. Amount above PAYER_MAX_PER_CALL
  if (amountBigInt > config.maxPerCall) {
    throw new PayerError(
      'AMOUNT_EXCEEDS_MAX_PER_CALL',
      `Payment amount ${amountBigInt.toString()} exceeds maximum per call limit of ${config.maxPerCall.toString()}`
    );
  }

  // 5. Asset or pay_to not present in the on-chain policy (query via get_payee_rule)
  if (!requirements.asset || !requirements.payTo) {
    throw new PayerError(
      'PAYEE_NOT_ALLOWLISTED',
      'Both asset and payTo must be present in payment requirements'
    );
  }

  // Validate addresses
  try {
    Address.fromString(requirements.asset);
    Address.fromString(requirements.payTo);
  } catch {
    throw new PayerError(
      'PAYEE_NOT_ALLOWLISTED',
      'Invalid asset or payTo address in payment requirements'
    );
  }

  const queryFn = queryPayeeRule || ((asset, payTo) => queryPayeeRuleOnChain(config, asset, payTo));
  const ruleId = await queryFn(requirements.asset, requirements.payTo);

  if (ruleId === null || ruleId === undefined) {
    throw new PayerError(
      'PAYEE_NOT_ALLOWLISTED',
      `Payee (${requirements.asset}, ${requirements.payTo}) is not allowlisted by on-chain spend policy`
    );
  }

  return { ruleId };
}
