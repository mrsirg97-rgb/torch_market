import Link from 'next/link'
import fs from 'fs'
import path from 'path'
import type { Metadata } from 'next'

export const metadata: Metadata = {
  title: 'torch.market Whitepaper',
  description:
    'Every token is its own margin market. Treasury-backed lending, real-token short selling, on-chain pricing. No oracles. No LPs. No bootstrapping.',
  openGraph: {
    title: 'torch.market Whitepaper',
    description:
      'Every token is its own margin market. Treasury-backed lending, real-token short selling, on-chain pricing.',
    url: 'https://torch.market/whitepaper',
    siteName: 'torch.market',
    type: 'article',
  },
  twitter: {
    card: 'summary_large_image',
    title: 'torch.market Whitepaper',
    description:
      'Every token is its own margin market. No oracles. No LPs. No bootstrapping.',
    creator: '@torch_market',
  },
}

/** Minimal markdown-to-HTML for the whitepaper (no dependencies) */
function renderMarkdown(md: string): string {
  let html = md
    // Remove YAML frontmatter if present
    .replace(/^---[\s\S]*?---\n*/m, '')

  // Code blocks (``` ... ```)
  html = html.replace(/```([\s\S]*?)```/g, (_m, code) => {
    return `<pre><code>${code.replace(/</g, '&lt;').replace(/>/g, '&gt;').trim()}</code></pre>`
  })

  // Tables
  html = html.replace(/^\|(.+)\|\n\|[-| :]+\|\n((?:\|.+\|\n)*)/gm, (_m, headerRow, bodyRows) => {
    const headers = headerRow.split('|').map((h: string) => h.trim()).filter(Boolean)
    const headerHtml = headers.map((h: string) => `<th>${h}</th>`).join('')
    const rows = bodyRows.trim().split('\n').map((row: string) => {
      const cells = row.split('|').map((c: string) => c.trim()).filter(Boolean)
      return `<tr>${cells.map((c: string) => `<td>${c}</td>`).join('')}</tr>`
    }).join('')
    return `<table><thead><tr>${headerHtml}</tr></thead><tbody>${rows}</tbody></table>`
  })

  // Process line by line for headings, paragraphs, lists
  const lines = html.split('\n')
  const result: string[] = []
  let inList = false

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]

    // Headings
    if (line.startsWith('# ')) {
      if (inList) { result.push('</ul>'); inList = false }
      result.push(`<h1>${line.slice(2)}</h1>`)
    } else if (line.startsWith('## ')) {
      if (inList) { result.push('</ul>'); inList = false }
      result.push(`<h2>${line.slice(3)}</h2>`)
    } else if (line.startsWith('### ')) {
      if (inList) { result.push('</ul>'); inList = false }
      result.push(`<h3>${line.slice(4)}</h3>`)
    } else if (line.startsWith('- ')) {
      if (!inList) { result.push('<ul>'); inList = true }
      result.push(`<li>${line.slice(2)}</li>`)
    } else if (line.startsWith('> ')) {
      if (inList) { result.push('</ul>'); inList = false }
      result.push(`<blockquote>${line.slice(2)}</blockquote>`)
    } else if (line === '---') {
      if (inList) { result.push('</ul>'); inList = false }
      result.push('<hr />')
    } else if (line.trim() === '') {
      if (inList) { result.push('</ul>'); inList = false }
    } else if (!line.startsWith('<')) {
      // Regular paragraph (skip lines already HTML from table/code processing)
      if (inList) { result.push('</ul>'); inList = false }
      result.push(`<p>${line}</p>`)
    } else {
      result.push(line)
    }
  }
  if (inList) result.push('</ul>')

  // Inline formatting
  let output = result.join('\n')
  output = output.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
  output = output.replace(/\*(.+?)\*/g, '<em>$1</em>')
  output = output.replace(/`([^`]+)`/g, '<code>$1</code>')
  output = output.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>')

  return output
}

export default function WhitepaperPage() {
  const mdPath = path.join(process.cwd(), 'public', 'whitepaper.md')
  const markdown = fs.readFileSync(mdPath, 'utf-8')
  const html = renderMarkdown(markdown)

  return (
    <main className="min-h-screen bg-white text-[#1a1a1a]">
      <div className="max-w-3xl mx-auto px-6 sm:px-8 py-12 sm:py-16">
        {/* Back link */}
        <div className="mb-8">
          <Link
            href="/"
            className="text-sm text-[#888] hover:text-[#1a1a1a] transition-colors"
          >
            &larr; torch.market
          </Link>
        </div>

        {/* Rendered markdown */}
        <article
          className="whitepaper-content"
          dangerouslySetInnerHTML={{ __html: html }}
        />

        {/* Footer */}
        <div className="mt-16 pt-8 border-t border-[#e5e5e5] text-center text-sm text-[#888]">
          <p>&copy; 2026 Brightside Solutions. All rights reserved.</p>
          <div className="flex gap-4 justify-center mt-2">
            <a href="https://torch.market/terms" className="hover:text-[#1a1a1a]">Terms</a>
            <a href="https://torch.market/privacy" className="hover:text-[#1a1a1a]">Privacy</a>
            <Link href="/" className="hover:text-[#1a1a1a]">torch.market</Link>
          </div>
        </div>
      </div>

      {/* Scoped whitepaper styles */}
      <style>{`
        .whitepaper-content h1 {
          font-size: 2rem;
          font-weight: 700;
          margin: 2.5rem 0 0.75rem;
          color: #1a1a1a;
          letter-spacing: -0.02em;
        }
        .whitepaper-content h1:first-child {
          font-size: 2.5rem;
          margin-top: 0;
        }
        .whitepaper-content h2 {
          font-size: 1.4rem;
          font-weight: 600;
          margin: 2rem 0 0.5rem;
          color: #1a1a1a;
          letter-spacing: -0.01em;
        }
        .whitepaper-content h3 {
          font-size: 1.1rem;
          font-weight: 600;
          margin: 1.5rem 0 0.5rem;
          color: #1a1a1a;
        }
        .whitepaper-content p {
          font-size: 0.95rem;
          line-height: 1.7;
          color: #333;
          margin: 0.5rem 0;
        }
        .whitepaper-content strong {
          color: #1a1a1a;
          font-weight: 600;
        }
        .whitepaper-content a {
          color: #ea580c;
          text-decoration: none;
        }
        .whitepaper-content a:hover {
          text-decoration: underline;
        }
        .whitepaper-content hr {
          border: none;
          border-top: 1px solid #e5e5e5;
          margin: 2rem 0;
        }
        .whitepaper-content ul {
          margin: 0.5rem 0;
          padding-left: 1.5rem;
        }
        .whitepaper-content li {
          font-size: 0.95rem;
          line-height: 1.7;
          color: #333;
          margin: 0.25rem 0;
        }
        .whitepaper-content blockquote {
          border-left: 3px solid #e5e5e5;
          padding-left: 1rem;
          margin: 1rem 0;
          color: #666;
          font-size: 0.9rem;
        }
        .whitepaper-content code {
          font-family: var(--font-space-mono), monospace;
          font-size: 0.85rem;
          background: #f5f5f4;
          padding: 0.15rem 0.4rem;
          border-radius: 3px;
          color: #1a1a1a;
        }
        .whitepaper-content pre {
          background: #f5f5f4;
          border: 1px solid #e5e5e5;
          border-radius: 6px;
          padding: 1rem;
          overflow-x: auto;
          margin: 1rem 0;
        }
        .whitepaper-content pre code {
          background: none;
          padding: 0;
          font-size: 0.8rem;
          line-height: 1.5;
          color: #333;
        }
        .whitepaper-content table {
          width: 100%;
          border-collapse: collapse;
          margin: 1rem 0;
          font-size: 0.85rem;
        }
        .whitepaper-content th {
          text-align: left;
          padding: 0.5rem 0.75rem;
          border-bottom: 2px solid #e5e5e5;
          color: #1a1a1a;
          font-weight: 600;
        }
        .whitepaper-content td {
          padding: 0.5rem 0.75rem;
          border-bottom: 1px solid #f0f0f0;
          color: #333;
        }
      `}</style>
    </main>
  )
}
