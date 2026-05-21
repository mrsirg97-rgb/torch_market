'use client'

import { shortenAddress } from '@/lib/constants'
import { TokenMessage } from '@/hooks/useToken'

interface MessageFeedProps {
  messages: TokenMessage[]
}

export function MessageFeed({ messages }: MessageFeedProps) {
  return (
    <div className="card p-6 lg:col-span-2">
      <h2 className="font-semibold mb-4 flex items-center gap-2">
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="18"
          height="18"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
        </svg>
        Messages
        <span className="text-white/40 text-sm font-normal">({messages.length})</span>
      </h2>

      {messages.length > 0 ? (
        <div className="space-y-3 max-h-[500px] overflow-y-auto">
          {messages.map((msg) => (
            <div key={msg.signature} className="bg-white/5 rounded-lg p-3">
              <div className="flex items-start justify-between gap-2 mb-1">
                <span className="text-accent text-xs font-mono">{shortenAddress(msg.sender)}</span>
                <span className="text-white/30 text-xs">
                  {msg.timestamp
                    ? new Date(msg.timestamp * 1000).toLocaleString(undefined, {
                        month: 'short',
                        day: 'numeric',
                        hour: '2-digit',
                        minute: '2-digit',
                      })
                    : ''}
                </span>
              </div>
              <p className="text-white text-sm">{msg.memo}</p>
              <a
                href={`https://solscan.io/tx/${msg.signature}`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-white/30 hover:text-white/50 text-xs mt-1 inline-block"
              >
                View tx
              </a>
            </div>
          ))}
        </div>
      ) : (
        <div className="text-center py-8">
          <p className="text-white/40 text-sm">No messages yet</p>
          <p className="text-white/30 text-xs mt-1">
            Trade with a message to start the conversation
          </p>
        </div>
      )}
    </div>
  )
}
