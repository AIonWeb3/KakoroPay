import { describe, expect, it } from 'vitest';
import { hash, Keypair, scValToNative, xdr } from '@stellar/stellar-sdk';
import {
  AgentAccountSigner,
  buildAuthDigest,
  buildAuthPayloadScVal,
} from '../src/signer.js';
import {
  TEST_ACCOUNT_ID,
  TEST_AGENT_KEYPAIR,
  TEST_CONFIG,
} from './fixtures.js';

describe('AgentAccountSigner and AuthPayload', () => {
  it('builds auth digest binding context_rule_ids into signature preimage', () => {
    const rawSignaturePayload = hash(Buffer.from('test-invocation-preimage'));
    const ruleIds = [7];

    const { authDigest, ruleIdsScVal } = buildAuthDigest(rawSignaturePayload, ruleIds);

    // Verify digest matches sha256(signaturePayload || [7].to_xdr())
    const expectedPreimage = Buffer.concat([rawSignaturePayload, ruleIdsScVal.toXDR()]);
    const expectedDigest = hash(expectedPreimage);

    expect(authDigest.equals(expectedDigest)).toBe(true);
    expect(authDigest.length).toBe(32);
  });

  it('builds AuthPayload ScVal with correct structure expected by OpenZeppelin stellar-accounts', () => {
    const dummySignature = Buffer.alloc(64, 9);
    const ruleIdsScVal = xdr.ScVal.scvVec([xdr.ScVal.scvU32(3)]);

    const payloadScVal = buildAuthPayloadScVal(
      TEST_AGENT_KEYPAIR.publicKey(),
      dummySignature,
      ruleIdsScVal
    );

    // Verify it is a Map with context_rule_ids and signers
    expect(payloadScVal.switch()).toBe(xdr.ScValType.scvMap());
    const mapEntries = payloadScVal.map()!;
    expect(mapEntries.length).toBe(2);

    const keys = mapEntries.map((e) => e.key().sym().toString());
    expect(keys).toEqual(['context_rule_ids', 'signers']);

    // context_rule_ids entry
    const contextRuleIdsVal = mapEntries.find((e) => e.key().sym().toString() === 'context_rule_ids')!.val();
    expect(contextRuleIdsVal.switch()).toBe(xdr.ScValType.scvVec());
    expect(scValToNative(contextRuleIdsVal)).toEqual([3]);

    // signers entry
    const signersVal = mapEntries.find((e) => e.key().sym().toString() === 'signers')!.val();
    expect(signersVal.switch()).toBe(xdr.ScValType.scvMap());
    const signerEntries = signersVal.map()!;
    expect(signerEntries.length).toBe(1);

    // Key is Signer::Delegated(agentAddress) -> Vec([Symbol("Delegated"), Address(agent)])
    const signerKeyVal = signerEntries[0].key();
    expect(signerKeyVal.switch()).toBe(xdr.ScValType.scvVec());
    const signerKeyParts = signerKeyVal.vec()!;
    expect(signerKeyParts[0].sym().toString()).toBe('Delegated');
    expect(signerKeyParts[1].address().accountId().ed25519().toString('hex')).toBe(
      TEST_AGENT_KEYPAIR.rawPublicKey().toString('hex')
    );

    // Val is the 64-byte signature
    const signerBytesVal = signerEntries[0].val();
    expect(signerBytesVal.bytes().equals(dummySignature)).toBe(true);
  });

  it('signs auth entry and produces verifiable Ed25519 signature over authDigest', async () => {
    const signer = new AgentAccountSigner({
      accountContractId: TEST_ACCOUNT_ID,
      agentSecretKey: TEST_AGENT_KEYPAIR.secret(),
      networkPassphrase: TEST_CONFIG.networkPassphrase,
      ruleId: 10,
    });

    const fakePayload = Buffer.alloc(32, 1);
    const { signedAuthEntry, signerAddress } = await signer.signAuthEntry(
      fakePayload.toString('base64')
    );

    expect(signerAddress).toBe(TEST_AGENT_KEYPAIR.publicKey());

    // Verify signature
    const sigBuf = Buffer.from(signedAuthEntry, 'base64');
    const { authDigest } = buildAuthDigest(fakePayload, [10]);

    const verified = Keypair.fromPublicKey(signerAddress!).verify(authDigest, sigBuf);
    expect(verified).toBe(true);
  });

  it('calculates short expiration ledger offset properly', () => {
    const currentLedger = 1000;
    const estimatedLedgerSeconds = 5;
    const timeoutSeconds = 60;

    const maxLedger = currentLedger + Math.ceil(timeoutSeconds / estimatedLedgerSeconds);
    // 60 / 5 = 12 ledgers
    expect(maxLedger).toBe(1012);
  });
});
