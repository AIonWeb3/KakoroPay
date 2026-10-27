import { Buffer } from 'node:buffer';
import {
  Address,
  hash,
  Keypair,
  xdr,
} from '@stellar/stellar-sdk';
import type { SignAuthEntry } from '@stellar/stellar-sdk/contract';

export interface AgentAccountSignerOptions {
  accountContractId: string;
  agentSecretKey: string;
  networkPassphrase: string;
  ruleId: number;
  expirationLedger?: number;
}

let activeSignatureScVal: xdr.ScVal | null = null;
let hookInstalled = false;

function ensureBufferSignatureScValHook() {
  if (!hookInstalled) {
    Object.defineProperty(Buffer.prototype, 'signatureScVal', {
      get() {
        return activeSignatureScVal ?? undefined;
      },
      configurable: true,
    });
    hookInstalled = true;
  }
}

export function setActiveSignatureScVal(scVal: xdr.ScVal | null) {
  ensureBufferSignatureScValHook();
  activeSignatureScVal = scVal;
}

export function clearActiveSignatureScVal() {
  activeSignatureScVal = null;
}

/**
 * Builds the digest that signers authenticate against in OpenZeppelin stellar-accounts 0.7.x:
 * auth_digest = sha256(signature_payload || context_rule_ids.to_xdr())
 */
export function buildAuthDigest(
  signaturePayload: Buffer,
  ruleIds: number[]
): { authDigest: Buffer; ruleIdsScVal: xdr.ScVal } {
  const ruleIdsScVal = xdr.ScVal.scvVec(ruleIds.map((id) => xdr.ScVal.scvU32(id)));
  const ruleIdsXdr = ruleIdsScVal.toXDR();
  const preimage = Buffer.concat([signaturePayload, ruleIdsXdr]);
  return {
    authDigest: hash(preimage),
    ruleIdsScVal,
  };
}

/**
 * Builds the AuthPayload structure expected by OpenZeppelin stellar-accounts:
 * struct AuthPayload {
 *   signers: Map<Signer, Bytes>,
 *   context_rule_ids: Vec<u32>,
 * }
 */
export function buildAuthPayloadScVal(
  agentPublicKey: string,
  signature: Buffer,
  ruleIdsScVal: xdr.ScVal
): xdr.ScVal {
  // Signer::Delegated(agentAddress) -> ScVal::Vec([Symbol("Delegated"), Address(agent)])
  const signerKey = xdr.ScVal.scvVec([
    xdr.ScVal.scvSymbol('Delegated'),
    new Address(agentPublicKey).toScVal(),
  ]);
  const signerVal = xdr.ScVal.scvBytes(signature);

  return xdr.ScVal.scvMap([
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol('context_rule_ids'),
      val: ruleIdsScVal,
    }),
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol('signers'),
      val: xdr.ScVal.scvMap([
        new xdr.ScMapEntry({
          key: signerKey,
          val: signerVal,
        }),
      ]),
    }),
  ]);
}

/**
 * ClientStellarSigner implementation backed by the AgentPay smart account contract.
 */
export class AgentAccountSigner {
  readonly address: string;
  readonly agentKeypair: Keypair;
  readonly networkPassphrase: string;
  readonly ruleId: number;
  readonly expirationLedger?: number;

  constructor(options: AgentAccountSignerOptions) {
    this.address = options.accountContractId;
    this.agentKeypair = Keypair.fromSecret(options.agentSecretKey);
    this.networkPassphrase = options.networkPassphrase;
    this.ruleId = options.ruleId;
    this.expirationLedger = options.expirationLedger;
  }

  signAuthEntry: SignAuthEntry = async (authEntry: string, _opts?: { networkPassphrase?: string; address?: string }) => {
    let signaturePayload: Buffer;
    try {
      const preimage = xdr.HashIdPreimage.fromXDR(authEntry, 'base64');
      signaturePayload = hash(preimage.toXDR());
    } catch {
      // If raw 32-byte payload or raw authorization entry
      try {
        const rawBuf = Buffer.from(authEntry, 'base64');
        if (rawBuf.length === 32) {
          signaturePayload = rawBuf;
        } else {
          const entry = xdr.SorobanAuthorizationEntry.fromXDR(authEntry, 'base64');
          signaturePayload = hash(entry.toXDR());
        }
      } catch {
        signaturePayload = hash(Buffer.from(authEntry, 'base64'));
      }
    }

    // Bind rule ID into auth digest
    const { authDigest, ruleIdsScVal } = buildAuthDigest(signaturePayload, [this.ruleId]);

    // Sign with agent's ed25519 key
    const signature = this.agentKeypair.sign(authDigest);

    // Build OpenZeppelin AuthPayload ScVal
    const authPayloadScVal = buildAuthPayloadScVal(
      this.agentKeypair.publicKey(),
      signature,
      ruleIdsScVal
    );

    // Set signatureScVal hook for authorizeEntry in @stellar/stellar-sdk
    setActiveSignatureScVal(authPayloadScVal);

    return {
      signedAuthEntry: signature.toString('base64'),
      signerAddress: this.agentKeypair.publicKey(),
    };
  };
}
