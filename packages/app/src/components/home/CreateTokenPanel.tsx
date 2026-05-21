'use client'

import { useState } from 'react'
import { useRouter } from 'next/navigation'
import Image from 'next/image'
import { useConnection, useWallet } from '@solana/wallet-adapter-react'
import { sendCreateToken } from 'torchsdk'
import { uploadTokenAssets } from '@/lib/irys'
import { TokenTier, TIER_CONFIG, SOL_TARGET_MAP } from '@/types/token'

interface CreateTokenPanelProps {
  isOpen: boolean
  onClose: () => void
}

/**
 * Slide-over panel for creating new tokens
 */
export function CreateTokenPanel({ isOpen, onClose }: CreateTokenPanelProps) {
  const router = useRouter()
  const { connection } = useConnection()
  const wallet = useWallet()


  // Form state
  const [name, setName] = useState('')
  const [symbol, setSymbol] = useState('')
  const [description, setDescription] = useState('')
  const [imageFile, setImageFile] = useState<File | null>(null)
  const [imagePreview, setImagePreview] = useState<string | null>(null)
  const [selectedTier, setSelectedTier] = useState<TokenTier>('flame')
  const [creatorFees, setCreatorFees] = useState(false)
  const [twitter, setTwitter] = useState('')
  const [telegram, setTelegram] = useState('')
  const [website, setWebsite] = useState('')

  // Status state
  const [createLoading, setCreateLoading] = useState(false)
  const [createStatus, setCreateStatus] = useState<string | null>(null)
  const [createError, setCreateError] = useState<string | null>(null)

  function compressImage(file: File): Promise<File> {
    return new Promise((resolve, reject) => {
      const img = new window.Image()
      img.onload = () => {
        const canvas = document.createElement('canvas')
        const MAX_SIZE = 512
        let { width, height } = img
        if (width > MAX_SIZE || height > MAX_SIZE) {
          if (width > height) {
            height = Math.round((height * MAX_SIZE) / width)
            width = MAX_SIZE
          } else {
            width = Math.round((width * MAX_SIZE) / height)
            height = MAX_SIZE
          }
        }
        canvas.width = width
        canvas.height = height
        const ctx = canvas.getContext('2d')!
        ctx.drawImage(img, 0, 0, width, height)
        canvas.toBlob(
          (blob) => {
            if (!blob) return reject(new Error('Image compression failed'))
            const compressed = new File([blob], file.name.replace(/\.\w+$/, '.jpg'), {
              type: 'image/jpeg',
            })
            resolve(compressed)
          },
          'image/jpeg',
          0.8,
        )
      }
      img.onerror = () => reject(new Error('Failed to load image'))
      img.src = URL.createObjectURL(file)
    })
  }

  async function handleImageSelect(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0]
    if (!file) return

    // Validate file type
    if (!file.type.startsWith('image/')) {
      setCreateError('Please select an image file')
      return
    }

    // Validate file size (max 5MB)
    if (file.size > 5 * 1024 * 1024) {
      setCreateError('Image must be less than 5MB')
      return
    }

    try {
      // Compress to JPEG <=512px to keep Arweave uploads small and reliable
      const compressed = await compressImage(file)
      setImageFile(compressed)
      setCreateError(null)

      // Create preview
      const reader = new FileReader()
      reader.onload = (e) => {
        setImagePreview(e.target?.result as string)
      }
      reader.readAsDataURL(compressed)
    } catch {
      setCreateError('Failed to process image. Try a different file.')
    }
  }

  async function handleCreate() {
    if (!wallet.publicKey || !wallet.signMessage) {
      setCreateError('Please connect your wallet (must support message signing)')
      return
    }

    if (!name || !symbol) {
      setCreateError('Name and symbol are required')
      return
    }

    if (!imageFile) {
      setCreateError('market image is required')
      return
    }

    if (name.length > 32) {
      setCreateError('Name must be 32 characters or less')
      return
    }

    if (symbol.length > 10) {
      setCreateError('Symbol must be 10 characters or less')
      return
    }

    // Validate URLs if provided
    const urlFields = [
      { value: twitter, name: 'Twitter' },
      { value: telegram, name: 'Telegram' },
      { value: website, name: 'Website' },
    ]
    for (const field of urlFields) {
      if (field.value) {
        try {
          new URL(field.value)
        } catch {
          setCreateError(`Invalid ${field.name} URL format`)
          return
        }
      }
    }

    setCreateLoading(true)
    setCreateError(null)

    try {
      // Step 1: Upload image and metadata to Arweave
      setCreateStatus('Uploading image to Arweave...')
      const { metadataUrl } = await uploadTokenAssets(
        {
          publicKey: wallet.publicKey,
          signMessage: wallet.signMessage,
          signTransaction: wallet.signTransaction,
        },
        imageFile,
        {
          name,
          symbol,
          description: description || undefined,
          twitter: twitter || undefined,
          telegram: telegram || undefined,
          website: website || undefined,
        },
      )

      // Step 2: Create token via SDK (builds, simulates, and sends in one call)
      setCreateStatus('Please approve in wallet...')
      const { signature, mint: mintPubkey } = await sendCreateToken(connection, {
        publicKey: wallet.publicKey!,
        signAndSendTransaction: async (tx) => {
          const sig = await wallet.sendTransaction!(tx, connection)
          return { signature: sig }
        },
      }, {
        name,
        symbol,
        metadata_uri: metadataUrl,
        sol_target: SOL_TARGET_MAP[selectedTier],
        community_token: !creatorFees,
      })

      // Confirm transaction before showing success
      setCreateStatus('Confirming transaction...')
      const confirmation = await connection.confirmTransaction(signature, 'confirmed')
      if (confirmation.value.err) {
        throw new Error('Transaction failed on-chain')
      }

      // Store mint address before resetting
      const mintAddress = mintPubkey.toString()

      // Reset form and close panel
      resetForm()
      onClose()

      // Navigate to the new token page
      router.push(`/markets/${mintAddress}`)
    } catch (err: unknown) {
      const error = err as { message?: string; logs?: string[]; error?: { logs?: string[] } }
      setCreateError(error.message || 'failed to create market')
      setCreateStatus(null)
    } finally {
      setCreateLoading(false)
    }
  }

  function resetForm() {
    setName('')
    setSymbol('')
    setDescription('')
    setImageFile(null)
    setImagePreview(null)
    setSelectedTier('flame')
    setCreatorFees(false)
    setTwitter('')
    setTelegram('')
    setWebsite('')
    setCreateStatus(null)
    setCreateError(null)
  }

  return (
    <>
      {/* Slide-over Panel */}
      <div
        className={`fixed top-0 right-0 h-full w-full md:w-[420px] backdrop-blur-xl transform transition-transform duration-300 ease-in-out z-[60] ${
          isOpen ? 'translate-x-0' : 'translate-x-full'
        }`}
        style={{
          background:
            'color-mix(in srgb, var(--background) 92%, transparent)',
          boxShadow: '-20px 0 60px rgba(0,0,0,0.25)',
        }}
      >
        <div className="h-full overflow-y-auto p-6">
          <div className="flex items-center justify-between mb-6">
            <h2 className="text-xl font-bold">launch market</h2>
            <button
              onClick={onClose}
              className="text-white/50 hover:text-white text-2xl cursor-pointer p-2 -mr-2 min-w-[44px] min-h-[44px] flex items-center justify-center"
              aria-label="Close panel"
            >
              &times;
            </button>
          </div>

          <p className="text-white/50 text-sm mb-6">
            Create a new market with automatic bonding curve, community treasury, and DEX migration.
          </p>

          <div className="space-y-5">
            {/* Image Upload */}
            <div>
              <label className="block text-sm text-white/70 mb-2">market image *</label>
              <div className="flex gap-4">
                <div
                  className={`w-24 h-24 rounded-lg border-2 border-dashed ${imagePreview ? 'border-accent' : 'border-white/20'} flex items-center justify-center overflow-hidden bg-white/5 cursor-pointer hover:border-accent/50 transition-colors`}
                  onClick={() => document.getElementById('image-upload')?.click()}
                >
                  {imagePreview ? (
                    <Image
                      src={imagePreview}
                      alt="Preview"
                      width={96}
                      height={96}
                      className="w-full h-full object-cover"
                      unoptimized
                    />
                  ) : (
                    <span className="text-white/30 text-3xl">+</span>
                  )}
                </div>
                <div className="flex-1">
                  <input
                    id="image-upload"
                    type="file"
                    accept="image/*"
                    onChange={handleImageSelect}
                    className="hidden"
                  />
                  <button
                    type="button"
                    onClick={() => document.getElementById('image-upload')?.click()}
                    className="btn btn-secondary text-sm mb-2"
                  >
                    {imageFile ? 'Change Image' : 'Select Image'}
                  </button>
                  <p className="text-white/40 text-xs">PNG, JPG, GIF up to 5MB. Image may take ~10 min to appear while settling on Arweave.</p>
                  {imageFile && <p className="text-accent text-xs mt-1">{imageFile.name}</p>}
                </div>
              </div>
            </div>

            {/* Name */}
            <div>
              <label className="block text-sm text-white/70 mb-2">market name *</label>
              <input
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g., My Awesome Market"
                maxLength={32}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              <p className="text-white/40 text-xs mt-1">{name.length}/32 characters</p>
            </div>

            {/* Symbol */}
            <div>
              <label className="block text-sm text-white/70 mb-2">market symbol *</label>
              <input
                type="text"
                value={symbol}
                onChange={(e) => setSymbol(e.target.value.toUpperCase())}
                placeholder="e.g., MAT"
                maxLength={8}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
              />
              <p className="text-white/40 text-xs mt-1">
                {symbol.length}/8 characters
                {symbol && <span className="text-white/60 ml-2">&rarr; ${symbol}</span>}
              </p>
            </div>

            {/* Description */}
            <div>
              <label className="block text-sm text-white/70 mb-2">Description</label>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Tell us about your market..."
                maxLength={500}
                rows={3}
                className="w-full bg-white/5 border border-white/10 rounded-lg px-4 py-3 text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors resize-none"
              />
              <p className="text-white/40 text-xs mt-1">{description.length}/500 characters</p>
            </div>

            {/* Tier Selector */}
            <div>
              <label className="block text-sm text-white/70 mb-2">Graduation Target *</label>
              <div className="grid grid-cols-3 gap-2">
                {(['flame', 'torch'] as TokenTier[]).map((tier) => {
                  const config = TIER_CONFIG[tier]
                  const isSelected = selectedTier === tier
                  return (
                    <button
                      key={tier}
                      type="button"
                      onClick={() => setSelectedTier(tier)}
                      className={`rounded-lg border px-3 py-3 text-center transition-all cursor-pointer ${
                        isSelected
                          ? 'border-accent bg-accent/10'
                          : 'border-white/10 bg-white/5 hover:border-white/20'
                      }`}
                    >
                      <span
                        className="block text-sm font-bold"
                        style={{ color: config.color }}
                      >
                        {config.label}
                      </span>
                      <span className="block text-white text-xs font-mono mt-0.5">
                        {config.solTarget} SOL
                      </span>
                      <span className="block text-white/40 text-[10px] mt-0.5">
                        {config.description}
                      </span>
                    </button>
                  )
                })}
              </div>
            </div>

            {/* Creator Fees Toggle */}
            <div>
              <button
                type="button"
                onClick={() => setCreatorFees(!creatorFees)}
                className={`w-full rounded-lg border px-4 py-3 text-left transition-all cursor-pointer ${
                  creatorFees
                    ? 'border-accent bg-accent/10'
                    : 'border-white/10 bg-white/5 hover:border-white/20'
                }`}
              >
                <div className="flex items-center justify-between">
                  <div>
                    <span className="text-sm font-medium text-white">
                      {creatorFees ? 'creator market' : 'community market'}
                    </span>
                    <p className="text-white/40 text-xs mt-0.5">
                      {creatorFees
                        ? 'You earn 0.2%→1% of buy SOL + 15% of post-migration fee proceeds'
                        : 'All fees go to community treasury (default)'}
                    </p>
                  </div>
                  <div
                    className={`w-10 h-5 rounded-full transition-colors relative ${
                      creatorFees ? 'bg-accent' : 'bg-white/20'
                    }`}
                  >
                    <div
                      className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                        creatorFees ? 'translate-x-5' : 'translate-x-0.5'
                      }`}
                    />
                  </div>
                </div>
              </button>
            </div>

            {/* Social Links */}
            <div className="space-y-3">
              <label className="block text-sm text-white/70">Social Links</label>
              <div className="flex gap-2 items-center">
                <span className="text-white/50 text-sm w-20">Twitter</span>
                <input
                  type="text"
                  value={twitter}
                  onChange={(e) => setTwitter(e.target.value)}
                  placeholder="https://x.com/..."
                  className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
                />
              </div>
              <div className="flex gap-2 items-center">
                <span className="text-white/50 text-sm w-20">Telegram</span>
                <input
                  type="text"
                  value={telegram}
                  onChange={(e) => setTelegram(e.target.value)}
                  placeholder="https://t.me/..."
                  className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
                />
              </div>
              <div className="flex gap-2 items-center">
                <span className="text-white/50 text-sm w-20">Website</span>
                <input
                  type="text"
                  value={website}
                  onChange={(e) => setWebsite(e.target.value)}
                  placeholder="https://..."
                  className="flex-1 bg-white/5 border border-white/10 rounded-lg px-3 py-2 text-sm text-white placeholder:text-white/30 focus:outline-none focus:border-accent transition-colors"
                />
              </div>
            </div>

            {/* Error */}
            {createError && (
              <div className="bg-danger/10 border border-danger/20 rounded-lg p-3 text-danger text-sm">
                {createError}
              </div>
            )}

            {/* Status */}
            {createStatus && (
              <div className="bg-accent/10 border border-accent/20 rounded-lg p-3 text-accent text-sm">
                <div className="flex items-center gap-2">
                  <span className="animate-spin">&#9696;</span>
                  <span>{createStatus}</span>
                </div>
              </div>
            )}

            {/* Submit */}
            <button
              onClick={handleCreate}
              disabled={createLoading || !wallet.publicKey}
              className="btn btn-accent w-full py-3 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {createLoading ? (
                <span className="flex items-center justify-center gap-2">
                  <span className="animate-spin">&#9696;</span>
                  Creating...
                </span>
              ) : !wallet.publicKey ? (
                'Connect Wallet'
              ) : (
                <span className="flex items-center justify-center gap-2">
                  <span>🔥</span>
                  launch market
                </span>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* Overlay when panel is open */}
      {isOpen && (
        <div
          className="fixed inset-0 bg-black/30 z-[55]"
          onClick={onClose}
        />
      )}
    </>
  )
}
