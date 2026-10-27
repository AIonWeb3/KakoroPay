import { ExactStellarScheme } from '@x402/stellar/exact/client';
import { loadConfig } from './config.js';
import { validatePreSignGuards } from './guards.js';
import { AgentAccountSigner, clearActiveSignatureScVal } from './signer.js';
import {
  CliInput,
  CliOutput,
  PaymentRequirements,
  PayerConfig,
  PayerError,
} from './types.js';

const CONTRACT_ERROR_MAP: Record<number, string> = {
  1: 'AlreadyInitialized',
  2: 'NotInitialized',
  3: 'Unauthorized',
  4: 'EmptyPolicy',
  5: 'OverBudget',
  6: 'RateLimited',
  7: 'InvalidContext',
  8: 'AgentGuardExecutionDenied',
};

export function mapSimulationError(error: unknown): { code: string; message: string } {
  const errorMsg = error instanceof Error ? error.message : String(error);

  // Check for known contract error names in error string
  for (const name of Object.values(CONTRACT_ERROR_MAP)) {
    if (errorMsg.includes(name)) {
      return { code: name, message: `Contract simulation rejected auth: ${name}` };
    }
  }

  // Check for contract error numbers like Error(Contract, #5) or Contract(5) or Error(Contract, 5)
  const contractErrMatch = errorMsg.match(/(?:Contract|#)\s*,?\s*#?(\d+)/i) ||
    errorMsg.match(/Error\(Contract,\s*#?(\d+)\)/i);
  if (contractErrMatch) {
    const codeNum = Number.parseInt(contractErrMatch[1], 10);
    if (CONTRACT_ERROR_MAP[codeNum]) {
      const name = CONTRACT_ERROR_MAP[codeNum];
      return { code: name, message: `Contract simulation rejected auth: ${name} (code ${codeNum})` };
    }
  }

  // Check for specific denial reasons in simulation log
  if (errorMsg.toLowerCase().includes('guard_denied') || errorMsg.includes('AgentGuard')) {
    return {
      code: 'AgentGuardExecutionDenied',
      message: 'Contract simulation rejected auth: AgentGuardExecutionDenied',
    };
  }
  if (errorMsg.toLowerCase().includes('over_budget')) {
    return { code: 'OverBudget', message: 'Contract simulation rejected auth: OverBudget' };
  }
  if (errorMsg.toLowerCase().includes('rate_limited')) {
    return { code: 'RateLimited', message: 'Contract simulation rejected auth: RateLimited' };
  }
  if (errorMsg.toLowerCase().includes('invalid_context')) {
    return { code: 'InvalidContext', message: 'Contract simulation rejected auth: InvalidContext' };
  }

  return { code: 'SimFailed', message: errorMsg };
}

export interface ProcessPaymentOptions {
  config?: PayerConfig;
  queryPayeeRule?: (asset: string, payTo: string) => Promise<number | null>;
  schemeClient?: {
    createPaymentPayload: (
      x402Version: number,
      reqs: PaymentRequirements
    ) => Promise<{ x402Version: number; payload: { transaction: string; [k: string]: unknown } }>;
  };
}

export async function processPayment(
  input: CliInput,
  options?: ProcessPaymentOptions
): Promise<CliOutput> {
  try {
    const paymentReqs = input.payment_requirements || input.paymentRequirements;
    if (!paymentReqs) {
      throw new PayerError(
        'INVALID_CLI_INPUT',
        'Input JSON must contain payment_requirements object'
      );
    }

    const resourceUrl = input.resource_url || input.resourceUrl;

    const config = options?.config || loadConfig();

    // 1. Run all pre-sign guards
    const { ruleId } = await validatePreSignGuards({
      requirements: paymentReqs,
      resourceUrl,
      config,
      queryPayeeRule: options?.queryPayeeRule,
    });

    // 2. Prepare smart account signer
    const signer = new AgentAccountSigner({
      accountContractId: config.accountContractId,
      agentSecretKey: config.agentSecretKey,
      networkPassphrase: config.networkPassphrase,
      ruleId,
    });

    // Ensure fee sponsorship is set as required by exact scheme
    if (!paymentReqs.extra) {
      paymentReqs.extra = { areFeesSponsored: true };
    } else if (paymentReqs.extra.areFeesSponsored === undefined) {
      paymentReqs.extra.areFeesSponsored = true;
    }

    // Set maxTimeoutSeconds if missing
    if (!paymentReqs.maxTimeoutSeconds) {
      paymentReqs.maxTimeoutSeconds = config.expirationSeconds;
    }

    // 3. Create payment payload using @x402/stellar
    try {
      const schemeClient =
        options?.schemeClient ||
        new ExactStellarScheme(signer, { url: config.sorobanRpcUrl });

      const x402Version = 1;
      const result = await schemeClient.createPaymentPayload(
        x402Version,
        paymentReqs as unknown as Parameters<ExactStellarScheme['createPaymentPayload']>[1]
      );

      return {
        ok: true,
        payment_payload: result.payload as { transaction: string; [k: string]: unknown },
        rule_id: ruleId,
      };
    } catch (simError) {
      const { code, message } = mapSimulationError(simError);
      return {
        ok: false,
        code,
        message,
      };
    } finally {
      clearActiveSignatureScVal();
    }
  } catch (err) {
    if (err instanceof PayerError) {
      return {
        ok: false,
        code: err.code,
        message: err.message,
      };
    }
    const { code, message } = mapSimulationError(err);
    return {
      ok: false,
      code,
      message,
    };
  }
}
