export default function PrivacyPage() {
  return (
    <main className="min-h-screen bg-black text-white p-8 max-w-3xl mx-auto">
      <h1 className="text-3xl font-bold mb-8">Privacy Policy</h1>
      <p className="text-white/50 text-sm mb-8">Last updated: January 2025</p>

      <div className="space-y-6 text-white/70">
        <section>
          <h2 className="text-xl font-semibold text-white mb-3">1. Introduction</h2>
          <p>
            This Privacy Policy describes how Torch Market handles information when you use our
            decentralized application. We are committed to protecting your privacy and being
            transparent about our practices.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">2. Information We Collect</h2>
          <p>Torch Market is a decentralized application. We collect minimal information:</p>
          <ul className="list-disc list-inside mt-2 space-y-1">
            <li>
              <strong>Wallet Address:</strong> Your public wallet address when you connect to the
              platform
            </li>
            <li>
              <strong>Transaction Data:</strong> All transactions are recorded on the public Solana
              blockchain
            </li>
            <li>
              <strong>Usage Data:</strong> Basic analytics about platform usage (page views, feature
              usage)
            </li>
          </ul>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">
            3. Information We Do Not Collect
          </h2>
          <ul className="list-disc list-inside mt-2 space-y-1">
            <li>
              Personal identification information (name, email, phone) unless voluntarily provided
            </li>
            <li>Private keys or seed phrases</li>
            <li>Financial information beyond public blockchain data</li>
          </ul>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">4. Blockchain Data</h2>
          <p>
            All transactions on Torch Market are executed on the Solana blockchain and are publicly
            visible. This includes token creation, trades, and treasury operations. This data cannot
            be deleted or modified due to the nature of blockchain technology.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">5. Third-Party Services</h2>
          <p>We may use third-party services for:</p>
          <ul className="list-disc list-inside mt-2 space-y-1">
            <li>RPC nodes (Solana network access)</li>
            <li>IPFS/Arweave (decentralized storage for token metadata)</li>
            <li>Analytics (anonymized usage data)</li>
          </ul>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">6. Cookies</h2>
          <p>
            We may use local storage to save your preferences (such as network selection). This data
            is stored locally on your device and is not transmitted to our servers.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">7. Security</h2>
          <p>
            We implement reasonable security measures to protect the platform. However, no internet
            transmission is completely secure. You are responsible for maintaining the security of
            your wallet and private keys.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">8. Changes to This Policy</h2>
          <p>
            We may update this Privacy Policy from time to time. Changes will be posted on this page
            with an updated revision date.
          </p>
        </section>

        <section>
          <h2 className="text-xl font-semibold text-white mb-3">9. Contact</h2>
          <p>
            For privacy-related questions, contact us at{' '}
            <a href="mailto:mrbrightsidecorp@gmail.com" className="text-accent hover:underline">
              mrbrightsidecorp@gmail.com
            </a>
          </p>
        </section>
      </div>
    </main>
  )
}
