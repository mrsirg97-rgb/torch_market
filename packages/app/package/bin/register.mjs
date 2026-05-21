#!/usr/bin/env node

import { Connection, Keypair, PublicKey, SystemProgram, Transaction, TransactionInstruction } from '@solana/web3.js';
import fs from 'fs';
import path from 'path';
import os from 'os';

// SAID Program on Solana Mainnet
const SAID_PROGRAM_ID = new PublicKey('5dpw6KEQPn248pnkkaYyWfHwu2nfb3LUMbTucb6LaA8G');
const TREASURY_PDA = new PublicKey('2XfHTeNWTjNwUmgoXaafYuqHcAAXj8F5Kjw2Bnzi4FxH');
const DEFAULT_RPC = 'https://api.mainnet-beta.solana.com';

function parseArgs() {
  const args = process.argv.slice(2);
  const options = {
    metadata: null,
    name: null,
    description: null,
    twitter: null,
    website: null,
    keypair: path.join(os.homedir(), '.config/solana/id.json'),
    rpc: DEFAULT_RPC,
    verify: false,
    help: false
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--metadata' || arg === '-m') {
      options.metadata = args[++i];
    } else if (arg === '--name' || arg === '-n') {
      options.name = args[++i];
    } else if (arg === '--description' || arg === '-d') {
      options.description = args[++i];
    } else if (arg === '--twitter' || arg === '-t') {
      options.twitter = args[++i];
    } else if (arg === '--website' || arg === '-w') {
      options.website = args[++i];
    } else if (arg === '--keypair' || arg === '-k') {
      options.keypair = args[++i];
    } else if (arg === '--rpc' || arg === '-r') {
      options.rpc = args[++i];
    } else if (arg === '--verify' || arg === '-v') {
      options.verify = true;
    } else if (arg === '--help' || arg === '-h') {
      options.help = true;
    }
  }

  return options;
}

function showHelp() {
  console.log(`
SAID Register - Register your AI agent on SAID Protocol

Usage:
  npx said-register --name <name> [options]
  npx said-register --metadata <url> [options]

Options:
  -n, --name <name>          Agent name (required if no --metadata)
  -d, --description <desc>   Agent description
  -t, --twitter <handle>     Twitter handle (e.g., @myagent)
  -w, --website <url>        Website URL
  -m, --metadata <url>       URL to AgentCard JSON (alternative to --name)
  -k, --keypair <path>       Path to Solana keypair (default: ~/.config/solana/id.json)
  -r, --rpc <url>            Solana RPC URL (default: mainnet)
  -v, --verify               Also get verified (costs 0.01 SOL)
  -h, --help                 Show this help message

Examples:
  # Simple registration with just a name
  npx said-register --name "MyAgent" --keypair wallet.json

  # Full registration with all details
  npx said-register --name "MyAgent" --description "AI trading bot" --twitter "@myagent" --keypair wallet.json

  # Using external metadata URL
  npx said-register --metadata https://example.com/agent.json --keypair wallet.json

Cost: ~0.03 SOL for registration, +0.01 SOL if using --verify

More info: https://www.saidprotocol.com
`);
}

function createDataUri(options) {
  const metadata = {
    name: options.name,
    description: options.description || `${options.name} - SAID verified agent`,
  };
  
  if (options.twitter) {
    metadata.twitter = options.twitter;
  }
  if (options.website) {
    metadata.website = options.website;
  }
  
  const json = JSON.stringify(metadata);
  const base64 = Buffer.from(json).toString('base64');
  return `data:application/json;base64,${base64}`;
}

function loadKeypair(keypairPath) {
  const resolved = keypairPath.startsWith('~') 
    ? path.join(os.homedir(), keypairPath.slice(1))
    : keypairPath;
    
  if (!fs.existsSync(resolved)) {
    throw new Error(`Keypair not found: ${resolved}`);
  }
  
  const secret = JSON.parse(fs.readFileSync(resolved, 'utf-8'));
  return Keypair.fromSecretKey(new Uint8Array(secret));
}

function deriveAgentPDA(owner) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from('agent'), owner.toBuffer()],
    SAID_PROGRAM_ID
  );
}

// Anchor discriminators (first 8 bytes of sha256("global:<method_name>"))
const REGISTER_DISCRIMINATOR = Buffer.from([24, 148, 140, 128, 250, 165, 176, 165]);
const VERIFY_DISCRIMINATOR = Buffer.from([133, 161, 141, 48, 120, 198, 88, 250]);

function createRegisterInstruction(agentPDA, owner, metadataUri) {
  const uriBuffer = Buffer.from(metadataUri, 'utf-8');
  const uriLenBuffer = Buffer.alloc(4);
  uriLenBuffer.writeUInt32LE(uriBuffer.length, 0);
  
  const data = Buffer.concat([
    REGISTER_DISCRIMINATOR,
    uriLenBuffer,
    uriBuffer
  ]);

  return new TransactionInstruction({
    keys: [
      { pubkey: agentPDA, isSigner: false, isWritable: true },
      { pubkey: owner, isSigner: true, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    programId: SAID_PROGRAM_ID,
    data,
  });
}

function createVerifyInstruction(agentPDA, owner) {
  return new TransactionInstruction({
    keys: [
      { pubkey: agentPDA, isSigner: false, isWritable: true },
      { pubkey: TREASURY_PDA, isSigner: false, isWritable: true },
      { pubkey: owner, isSigner: true, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    programId: SAID_PROGRAM_ID,
    data: VERIFY_DISCRIMINATOR,
  });
}

async function main() {
  const options = parseArgs();

  if (options.help) {
    showHelp();
    process.exit(0);
  }

  // Determine metadata URI
  let metadataUri;
  if (options.metadata) {
    metadataUri = options.metadata;
  } else if (options.name) {
    metadataUri = createDataUri(options);
  } else {
    console.error('Error: Either --name or --metadata is required\n');
    showHelp();
    process.exit(1);
  }

  console.log('🤖 SAID Register\n');

  // Load keypair
  console.log(`Loading keypair from ${options.keypair}...`);
  let wallet;
  try {
    wallet = loadKeypair(options.keypair);
  } catch (e) {
    console.error(`Error: ${e.message}`);
    process.exit(1);
  }
  console.log(`Wallet: ${wallet.publicKey.toString()}`);

  // Connect to Solana
  console.log(`Connecting to ${options.rpc}...`);
  const connection = new Connection(options.rpc, 'confirmed');
  
  // Check balance
  const balance = await connection.getBalance(wallet.publicKey);
  const solBalance = balance / 1e9;
  console.log(`Balance: ${solBalance.toFixed(4)} SOL`);
  
  if (solBalance < 0.03) {
    console.error('Error: Insufficient balance. Need at least 0.03 SOL for registration.');
    process.exit(1);
  }

  // Derive PDA
  const [agentPDA] = deriveAgentPDA(wallet.publicKey);
  console.log(`Agent PDA: ${agentPDA.toString()}`);

  // Check if already registered
  const existingAccount = await connection.getAccountInfo(agentPDA);
  if (existingAccount) {
    console.log('\n⚠️  Agent already registered at this PDA.');
    console.log('If you want to update metadata, use the update instruction.');
    process.exit(0);
  }

  // Show what we're registering
  if (options.name) {
    console.log(`\nAgent Name: ${options.name}`);
    if (options.description) console.log(`Description: ${options.description}`);
    if (options.twitter) console.log(`Twitter: ${options.twitter}`);
    if (options.website) console.log(`Website: ${options.website}`);
  }
  console.log(`\nMetadata URI: ${metadataUri.substring(0, 80)}${metadataUri.length > 80 ? '...' : ''}`);
  console.log('\nBuilding registration transaction...');
  
  const tx = new Transaction();
  tx.add(createRegisterInstruction(agentPDA, wallet.publicKey, metadataUri));
  
  if (options.verify) {
    if (solBalance < 0.013) {
      console.error('Error: Insufficient balance for verification. Need at least 0.013 SOL.');
      process.exit(1);
    }
    console.log('Adding verification instruction (0.01 SOL)...');
    tx.add(createVerifyInstruction(agentPDA, wallet.publicKey));
  }

  // Send transaction
  console.log('Sending transaction...');
  try {
    const { blockhash } = await connection.getLatestBlockhash();
    tx.recentBlockhash = blockhash;
    tx.feePayer = wallet.publicKey;
    tx.sign(wallet);
    
    const sig = await connection.sendRawTransaction(tx.serialize());
    console.log(`Transaction sent: ${sig}`);
    
    console.log('Confirming...');
    await connection.confirmTransaction(sig, 'confirmed');
    
    console.log('\n✅ Agent registered successfully!');
    console.log(`\nView on Solscan: https://solscan.io/account/${agentPDA.toString()}`);
    console.log(`View on SAID: https://www.saidprotocol.com/agents.html?search=${wallet.publicKey.toString()}`);
    
    if (options.verify) {
      console.log('\n✅ Agent verified!');
    } else {
      console.log('\nTip: Run with --verify to get a verified badge (0.01 SOL)');
    }
  } catch (e) {
    console.error(`\nError: ${e.message}`);
    if (e.logs) {
      console.error('Logs:', e.logs);
    }
    process.exit(1);
  }
}

main().catch(console.error);
