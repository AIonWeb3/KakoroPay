import express, { Express, Request, Response } from 'express';
import { HTTPFacilitatorClient } from '@x402/core/server';
import { ExactStellarScheme } from '@x402/stellar/exact/server';
import { x402ResourceServer, paymentMiddleware } from '@x402/express';

export interface SellerConfig {
  port?: number;
  facilitatorUrl?: string;
  facilitatorApiKey?: string;
  asset?: string;
  allowedPayee?: string;
  disallowedPayee?: string;
  amount?: string;
  isMock?: boolean;
}

export const DEFAULT_TESTNET_ASSET =
  process.env.TESTNET_ASSET ||
  'CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA'; // USDC SAC on testnet

export const DEFAULT_ALLOWED_PAYEE =
  process.env.ALLOWED_PAYEE ||
  process.env.SELLER_PAY_TO ||
  'GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5'; // Default testnet payee

export const DEFAULT_DISALLOWED_PAYEE =
  process.env.DISALLOWED_PAYEE ||
  'GDISALLOWED00000000000000000000000000000000000000000000000';

export const DEFAULT_FACILITATOR_URL =
  process.env.FACILITATOR_URL || 'https://www.x402.org/facilitator';

export const DEFAULT_AMOUNT = process.env.PRICE_AMOUNT || '1000';

export async function createSellerApp(config?: SellerConfig): Promise<Express> {
  const facilitatorUrl = config?.facilitatorUrl || DEFAULT_FACILITATOR_URL;
  const facilitatorApiKey = config?.facilitatorApiKey || process.env.FACILITATOR_API_KEY;
  const asset = config?.asset || DEFAULT_TESTNET_ASSET;
  const allowedPayee = config?.allowedPayee || DEFAULT_ALLOWED_PAYEE;
  const disallowedPayee = config?.disallowedPayee || DEFAULT_DISALLOWED_PAYEE;
  const amount = config?.amount || DEFAULT_AMOUNT;
  const isMock = config?.isMock ?? (process.env.E2E_MOCK === '1' || process.env.E2E_MOCK === 'true');

  const app = express();

  // Free health check
  app.get('/health', (_req: Request, res: Response) => {
    res.json({
      status: 'ok',
      asset,
      allowedPayee,
      disallowedPayee,
      amount,
      facilitatorUrl,
    });
  });

  if (isMock) {
    // Mock x402 seller mode for local verification without live testnet relayer
    app.get('/api/data', (req: Request, res: Response) => {
      const paymentHeader = req.header('PAYMENT-SIGNATURE') || req.header('X-PAYMENT');
      if (!paymentHeader) {
        const reqObj = {
          x402Version: 2,
          error: 'Payment required',
          resource: { url: `http://${req.headers.host}/api/data` },
          accepts: [
            {
              scheme: 'exact',
              network: 'stellar:testnet',
              amount,
              asset,
              payTo: allowedPayee,
              maxTimeoutSeconds: 300,
              extra: { areFeesSponsored: true },
            },
          ],
        };
        const b64 = Buffer.from(JSON.stringify(reqObj)).toString('base64');
        res.setHeader('PAYMENT-REQUIRED', b64);
        res.setHeader('Cache-Control', 'no-store');
        return res.status(402).json({ error: 'Payment required' });
      }

      // Settle mock payment
      const mockTxHash = 'e2e0000000000000000000000000000000000000000000000000000000000001';
      const settlementObj = {
        txHash: mockTxHash,
        transaction: mockTxHash,
        success: true,
      };
      res.setHeader('PAYMENT-RESPONSE', Buffer.from(JSON.stringify(settlementObj)).toString('base64'));
      return res.status(200).json({
        data: 'premium-testnet-weather-data',
        message: 'Payment settled successfully on testnet',
        cost: amount,
        payee: allowedPayee,
      });
    });

    app.get('/api/disallowed', (req: Request, res: Response) => {
      const paymentHeader = req.header('PAYMENT-SIGNATURE') || req.header('X-PAYMENT');
      if (!paymentHeader) {
        const reqObj = {
          x402Version: 2,
          error: 'Payment required',
          resource: { url: `http://${req.headers.host}/api/disallowed` },
          accepts: [
            {
              scheme: 'exact',
              network: 'stellar:testnet',
              amount,
              asset,
              payTo: disallowedPayee,
              maxTimeoutSeconds: 300,
              extra: { areFeesSponsored: true },
            },
          ],
        };
        const b64 = Buffer.from(JSON.stringify(reqObj)).toString('base64');
        res.setHeader('PAYMENT-REQUIRED', b64);
        res.setHeader('Cache-Control', 'no-store');
        return res.status(402).json({ error: 'Payment required' });
      }
      return res.status(403).json({ error: 'Disallowed payee' });
    });

    return app;
  }

  // Official @x402/express setup with facilitator
  const facilitatorHeaders = facilitatorApiKey
    ? { Authorization: `Bearer ${facilitatorApiKey}` }
    : undefined;

  const facilitator = new HTTPFacilitatorClient({
    url: facilitatorUrl,
    headers: facilitatorHeaders,
  });

  const resourceServer = new x402ResourceServer([facilitator]).register(
    'stellar:testnet',
    new ExactStellarScheme()
  );

  await resourceServer.initialize();

  const routes = {
    'GET /api/data': {
      accepts: [
        {
          scheme: 'exact',
          price: {
            amount,
            asset,
          },
          network: 'stellar:testnet',
          payTo: allowedPayee,
        },
      ],
      description: 'Official x402 Paid Testnet Endpoint',
    },
    'GET /api/disallowed': {
      accepts: [
        {
          scheme: 'exact',
          price: {
            amount,
            asset,
          },
          network: 'stellar:testnet',
          payTo: disallowedPayee,
        },
      ],
      description: 'Off-policy x402 Paid Testnet Endpoint',
    },
  };

  app.use(paymentMiddleware(routes, resourceServer, undefined, undefined, false));

  app.get('/api/data', (_req: Request, res: Response) => {
    res.json({
      data: 'premium-testnet-weather-data',
      message: 'Payment settled successfully on testnet',
      cost: amount,
      payee: allowedPayee,
    });
  });

  app.get('/api/disallowed', (_req: Request, res: Response) => {
    res.json({
      data: 'unauthorized-data',
      payee: disallowedPayee,
    });
  });

  return app;
}

// Standalone execution entrypoint
if (process.argv[1] && process.argv[1].endsWith('seller.js')) {
  const port = Number(process.env.PORT) || 4020;
  createSellerApp()
    .then((app) => {
      app.listen(port, () => {
        console.log(`[x402 Seller] listening on http://127.0.0.1:${port}`);
      });
    })
    .catch((err) => {
      console.error('[x402 Seller] Failed to start:', err);
      process.exit(1);
    });
}
