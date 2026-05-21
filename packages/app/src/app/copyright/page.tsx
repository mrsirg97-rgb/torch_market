export default function CopyrightPage() {
  return (
    <main className="min-h-screen bg-black text-white p-8 max-w-3xl mx-auto">
      <h1 className="text-3xl font-bold mb-8">Copyright Notice</h1>
      <p className="text-white/50 text-sm mb-8">Last updated: January 2025</p>

      <div className="space-y-6 text-white/70">
        <section>
          <h2 className="text-xl font-semibold text-white mb-3">Copyright</h2>
          <p>Copyright {new Date().getFullYear()} Torch Market. All rights reserved.</p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">Ownership</h2>
          <p>
            The Torch Market name, logo, website design, and application code are the property of
            their respective owners. The platform is built on open-source technologies and the
            Solana blockchain.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">User Content</h2>
          <p>
            Tokens created on Torch Market are the responsibility of their creators. Token names,
            symbols, descriptions, and images are user-generated content. Torch Market does not
            claim ownership of user-created tokens or their associated metadata.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">Trademarks</h2>
          <p>
            Solana, Phantom, Raydium, and other third-party names and logos are trademarks of their
            respective owners. Their use on this platform does not imply endorsement.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">Open Source</h2>
          <p>
            Portions of this platform may utilize open-source software. Such software is used in
            accordance with their respective licenses. The use of open-source components does not
            grant any rights to Torch Market trademarks or branding.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">DMCA</h2>
          <p>
            If you believe content on this platform infringes your copyright, please contact us at{' '}
            <a href="mailto:mrbrightsidecorp@gmail.com" className="text-accent hover:underline">
              mrbrightsidecorp@gmail.com
            </a>{' '}
            with details of the alleged infringement.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">Contact</h2>
          <p>
            For copyright inquiries, contact us at{' '}
            <a href="mailto:mrbrightsidecorp@gmail.com" className="text-accent hover:underline">
              mrbrightsidecorp@gmail.com
            </a>
          </p>
        </section>
      </div>
    </main>
  )
}
