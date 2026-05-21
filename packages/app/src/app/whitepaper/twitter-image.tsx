import { ImageResponse } from 'next/og'

export const runtime = 'edge'

export const alt = 'torch.market Whitepaper'
export const size = {
  width: 1200,
  height: 630,
}
export const contentType = 'image/png'

export default async function Image() {
  return new ImageResponse(
    <div
      style={{
        background: '#0a0a0a',
        width: '100%',
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        padding: '60px',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          maxWidth: '900px',
        }}
      >
        <h1
          style={{
            fontSize: '72px',
            fontWeight: 'bold',
            color: 'white',
            margin: '0 0 16px 0',
            textAlign: 'center',
          }}
        >
          torch.market
        </h1>

        <p
          style={{
            fontSize: '32px',
            color: '#f97316',
            margin: '0 0 40px 0',
            textAlign: 'center',
          }}
        >
          Every token is its own margin market.
        </p>

        <div
          style={{
            display: 'flex',
            gap: '40px',
            color: 'rgba(255, 255, 255, 0.5)',
            fontSize: '22px',
          }}
        >
          <span>Lending</span>
          <span style={{ color: 'rgba(255,255,255,0.2)' }}>·</span>
          <span>Short Selling</span>
          <span style={{ color: 'rgba(255,255,255,0.2)' }}>·</span>
          <span>No Oracle</span>
          <span style={{ color: 'rgba(255,255,255,0.2)' }}>·</span>
          <span>On-Chain Pricing</span>
        </div>
      </div>

      <div
        style={{
          position: 'absolute',
          bottom: '40px',
          right: '60px',
          background: '#f97316',
          color: 'black',
          padding: '10px 20px',
          borderRadius: '6px',
          fontSize: '18px',
          fontWeight: 'bold',
        }}
      >
        WHITEPAPER
      </div>

      <div
        style={{
          position: 'absolute',
          bottom: '40px',
          left: '60px',
          color: 'rgba(255, 255, 255, 0.3)',
          fontSize: '16px',
        }}
      >
        Brightside Solutions, 2026
      </div>
    </div>,
    {
      ...size,
    },
  )
}
