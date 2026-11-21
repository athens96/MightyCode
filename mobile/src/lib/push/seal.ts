import { chacha20poly1305 } from '@noble/ciphers/chacha';
import { hkdf } from '@noble/hashes/hkdf';
import { sha256 } from '@noble/hashes/sha256';

import { fromBase64, utf8Encode } from '@/api/relay/crypto';

/**
 * Opening a host's push (docs/mobile-remote.md "푸시 알림", native/contracts/README.md
 * "Push notifications"). The key comes from this phone's device token for that host:
 * HKDF-SHA256(SHA-256(token), salt clientId, info "mighty-push-v1"). The host keeps exactly
 * that SHA-256, so neither side needs a new secret, and the relay and FCM only ever carry
 * the envelope: base64url(0x01 ‖ nonce(12) ‖ ciphertext ‖ tag(16)), AAD "mighty-push-v1:" + handle.
 */
export const PUSH_INFO = 'mighty-push-v1';
export const PUSH_VERSION = 1;
export const PUSH_ENVELOPE_MAX = 3072;
export const PUSH_REPLAY_CACHE_SIZE = 128;
export const PUSH_MAX_AGE_MS = 24 * 60 * 60 * 1000;
export const PUSH_MAX_FUTURE_SKEW_MS = 10 * 60 * 1000;
const NONCE_LENGTH = 12;
const TAG_LENGTH = 16;
const ENVELOPE = /^[A-Za-z0-9_-]+$/;
const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/i;

/**
 * Bounded LRU replay cache of recently processed push identifiers (`capacity` defaults to 128).
 * `add(id)` returns `true` the first time `id` is recorded and `false` on duplicates while
 * refreshing `id` as most-recently-seen.
 */
export class SeenPushIds {
  private readonly ids = new Set<string>();

  constructor(private readonly capacity = PUSH_REPLAY_CACHE_SIZE) {
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new RangeError('SeenPushIds capacity must be a positive integer');
    }
  }

  get size(): number {
    return this.ids.size;
  }

  has(id: string): boolean {
    return this.ids.has(id);
  }

  add(id: string): boolean {
    if (this.ids.has(id)) {
      this.ids.delete(id);
      this.ids.add(id);
      return false;
    }
    if (this.ids.size >= this.capacity) {
      const oldest = this.ids.values().next().value;
      if (oldest !== undefined) this.ids.delete(oldest);
    }
    this.ids.add(id);
    return true;
  }

  clear(): void {
    this.ids.clear();
  }
}

/**
 * Validates that an ISO-8601 push `at` timestamp is at most 24 hours old and at most 10 minutes
 * ahead of `nowMs`. Truncates sub-millisecond fractional digits so Hermes and V8 parse identically.
 */
export function isFreshPushTimestamp(at: string, nowMs: number = Date.now()): boolean {
  if (!Number.isFinite(nowMs) || !ISO_TIMESTAMP.test(at)) return false;
  const normalized = at.replace(/(\.\d{3})\d+/, '$1');
  const parsedMs = Date.parse(normalized);
  if (!Number.isFinite(parsedMs)) return false;
  const ageMs = nowMs - parsedMs;
  return ageMs <= PUSH_MAX_AGE_MS && ageMs >= -PUSH_MAX_FUTURE_SKEW_MS;
}

export function pushKey(deviceToken: string, clientId: string): Uint8Array {
  return hkdf(sha256, sha256(utf8Encode(deviceToken)), utf8Encode(clientId), utf8Encode(PUSH_INFO), 32);
}

/** The plaintext, or null for anything that is not this host's envelope for this handle. */
export function openPush(envelope: string, key: Uint8Array, handle: string): Uint8Array | null {
  if (envelope.length > PUSH_ENVELOPE_MAX || !ENVELOPE.test(envelope)) return null;
  let bytes: Uint8Array;
  try {
    bytes = fromBase64(envelope);
  } catch {
    return null;
  }
  if (bytes.length < 1 + NONCE_LENGTH + TAG_LENGTH || bytes[0] !== PUSH_VERSION) return null;
  try {
    const nonce = bytes.subarray(1, 1 + NONCE_LENGTH);
    return chacha20poly1305(key, nonce, utf8Encode(`${PUSH_INFO}:${handle}`)).decrypt(bytes.subarray(1 + NONCE_LENGTH));
  } catch {
    return null;
  }
}
