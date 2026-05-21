export default function TermsPage() {
  return (
    <main className="min-h-screen bg-black text-white p-8 max-w-3xl mx-auto">
      <h1 className="text-3xl font-bold mb-8">Terms of Service</h1>
      <p className="text-white/50 text-sm mb-8">Last updated: January 2025</p>

      <div className="space-y-6 text-white/70">
        <section>
          <h2 className="text-xl font-semibold text-white mb-3">1. Acceptance of Terms</h2>
          <p>
            By accessing or using Torch Market, you agree to be bound by these Terms of Service. If
            you do not agree to these terms, do not use the platform.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">2. Description of Service</h2>
          <p>
            Torch Market is a decentralized token launchpad built on the Solana blockchain. The
            platform enables users to create and trade tokens using bonding curves, with automatic
            treasury mechanics and DEX migration.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">3. User Responsibilities</h2>
          <p>You are responsible for:</p>
          <ul className="list-disc list-inside mt-2 space-y-1">
            <li>Maintaining the security of your wallet and private keys</li>
            <li>All transactions made through your wallet</li>
            <li>Complying with applicable laws in your jurisdiction</li>
            <li>Understanding the risks associated with cryptocurrency trading</li>
          </ul>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">4. Risks</h2>
          <p>
            Cryptocurrency trading involves significant risk. Token values can be highly volatile
            and you may lose some or all of your investment. Torch Market does not provide financial
            advice. You should consult with a qualified financial advisor before making any
            investment decisions.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">5. No Warranties</h2>
          <p>
            The platform is provided &quot;as is&quot; without warranties of any kind. We do not
            guarantee uninterrupted access, error-free operation, or that the platform will meet
            your specific requirements.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">6. Limitation of Liability</h2>
          <p>
            To the maximum extent permitted by law, Torch Market and its operators shall not be
            liable for any indirect, incidental, special, consequential, or punitive damages arising
            from your use of the platform.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">7. Changes to Terms</h2>
          <p>
            We reserve the right to modify these terms at any time. Continued use of the platform
            after changes constitutes acceptance of the new terms.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">8. Contact</h2>
          <p>
            For questions about these terms, contact us at{' '}
            <a href="mailto:mrbrightsidecorp@gmail.com" className="text-accent hover:underline">
              mrbrightsidecorp@gmail.com
            </a>
          </p>
        </section>
      </div>
    </main>
  )
}
