import type { Metadata, Viewport } from 'next'
import { Space_Grotesk, Space_Mono } from 'next/font/google'
import { Providers } from './providers'
import './globals.css'

const spaceGrotesk = Space_Grotesk({
  variable: '--font-space-grotesk',
  subsets: ['latin'],
})

const spaceMono = Space_Mono({
  variable: '--font-space-mono',
  subsets: ['latin'],
  weight: ['400', '700'],
})

export const viewport: Viewport = {
  themeColor: '#0a0a0a',
}

export const metadata: Metadata = {
  title: 'torch.market',
  description:
    'Every token is its own margin market. Launch, lend, and short — no oracles, no LPs, no bootstrapping.',
  manifest: '/manifest.json',
  appleWebApp: {
    capable: true,
    statusBarStyle: 'black-translucent',
    title: 'torch.market',
  },
  icons: {
    icon: [
      { url: '/favicon-32x32.png', sizes: '32x32', type: 'image/png' },
      { url: '/favicon-16x16.png', sizes: '16x16', type: 'image/png' },
    ],
    apple: '/apple-touch-icon.png',
  },
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang="en">
      <body className={`${spaceGrotesk.variable} ${spaceMono.variable} antialiased`}>
        <Providers>{children}</Providers>
        {/* Right-edge "notebook" shadow on every page. This used to appear only on
            /markets as an incidental bleed from the closed launch panel's box-shadow;
            now it's a deliberate global element. pointer-events-none; sits below the
            header (z-50) and modals so it never overlays a dialog. */}
        <div
          aria-hidden
          className="pointer-events-none fixed inset-y-0 right-0 z-40"
          style={{ width: '1px', boxShadow: '-20px 0 60px rgba(0,0,0,0.25)' }}
        />
      </body>
    </html>
  )
}
