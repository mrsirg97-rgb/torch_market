import { TurboFactory } from '@ardrive/turbo-sdk/web'
import { Transaction, VersionedTransaction } from '@solana/web3.js'
import { getNetwork } from './network'

const isDev = process.env.NODE_ENV === 'development'

// Token metadata interface
export interface TokenMetadata {
  name: string
  symbol: string
  description?: string
  image: string
  twitter?: string
  telegram?: string
  website?: string
}

// Wallet interface for Turbo uploads
export interface IrysWallet {
  publicKey: { toBytes(): Uint8Array; toString(): string }
  signMessage?: (message: Uint8Array) => Promise<Uint8Array>
  signTransaction?: <T extends Transaction | VersionedTransaction>(tx: T) => Promise<T>
}

/**
 * Wrap fetch to convert ReadableStream bodies to Blob.
 * Phantom mobile's WebView doesn't support streaming fetch (duplex: 'half'),
 * but the Turbo SDK always wraps uploads in a ReadableStream.
 * This shim converts the stream to a Blob before sending — same approach
 * the Turbo SDK uses for Firefox/Safari, but applied universally.
 */
function withBlobFetch<T>(fn: () => Promise<T>): Promise<T> {
  const originalFetch = window.fetch
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    if (init?.body instanceof ReadableStream) {
      const blob = await new Response(init.body).blob()
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { duplex: _, ...rest } = init as RequestInit & { duplex?: string }
      return originalFetch(input, { ...rest, body: blob })
    }
    return originalFetch(input, init)
  }
  return fn().finally(() => {
    window.fetch = originalFetch
  })
}

// Full upload flow: image + metadata
export async function uploadTokenAssets(
  wallet: IrysWallet,
  imageFile: File,
  metadata: Omit<TokenMetadata, 'image'>,
): Promise<{ imageUrl: string; metadataUrl: string }> {
  const network = getNetwork()

  const turbo = TurboFactory.authenticated({
    token: 'solana',
    walletAdapter: {
      publicKey: wallet.publicKey,
      signMessage: wallet.signMessage!,
      signTransaction: wallet.signTransaction!,
    },
    ...(network.id !== 'mainnet' && { gatewayUrl: 'https://turbo.ardrive.dev' }),
  })

  return withBlobFetch(async () => {
    // Upload image
    const imageBuffer = Buffer.from(await imageFile.arrayBuffer())
    const imageResult = await turbo.upload({
      data: imageBuffer,
      dataItemOpts: {
        tags: [{ name: 'Content-Type', value: imageFile.type }],
      },
    })
    const imageUrl = `https://arweave.net/${imageResult.id}`
    if (isDev) console.log('Image uploaded:', imageUrl)

    // Upload metadata with image URL
    const fullMetadata: TokenMetadata = { ...metadata, image: imageUrl }
    const metadataResult = await turbo.upload({
      data: Buffer.from(JSON.stringify(fullMetadata)),
      dataItemOpts: {
        tags: [{ name: 'Content-Type', value: 'application/json' }],
      },
    })
    const metadataUrl = `https://arweave.net/${metadataResult.id}`
    if (isDev) console.log('Metadata uploaded:', metadataUrl)

    return { imageUrl, metadataUrl }
  })
}
