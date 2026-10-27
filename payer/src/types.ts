export interface PaymentRequirements {
  scheme: string;
  network: string;
  asset: string;
  payTo: string;
  amount: string;
  extra?: {
    areFeesSponsored?: boolean;
    [key: string]: unknown;
  };
  maxTimeoutSeconds?: number;
}

export interface CliInput {
  payment_requirements?: PaymentRequirements;
  paymentRequirements?: PaymentRequirements;
  resource_url?: string;
  resourceUrl?: string;
}

export interface CliSuccess {
  ok: true;
  payment_payload: {
    transaction: string;
    [key: string]: unknown;
  };
  rule_id?: number;
  header_name?: string;
  header_value?: string;
}

export interface CliError {
  ok: false;
  code: string;
  message: string;
}

export type CliOutput = CliSuccess | CliError;

export interface PayerConfig {
  accountContractId: string;
  agentSecretKey: string;
  sorobanRpcUrl: string;
  network: string;
  networkPassphrase: string;
  maxPerCall: bigint;
  expirationSeconds: number;
}

export class PayerError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = 'PayerError';
    this.code = code;
    Object.setPrototypeOf(this, PayerError.prototype);
  }
}
