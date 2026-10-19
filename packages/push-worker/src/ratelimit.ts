import type { RateLimiter } from "./types";

/** Seconds a caller should wait; the binding's windows are at most 60 s. */
export const RETRY_AFTER_SECONDS = 60;

/**
 * True when the request may proceed. A missing binding (local dev, tests,
 * accounts without the feature) or a binding error lets the request through:
 * the limits are abuse protection, not a correctness guarantee.
 */
export async function allow(limiter: RateLimiter | undefined, key: string): Promise<boolean> {
  if (!limiter) return true;
  try {
    const { success } = await limiter.limit({ key });
    return success;
  } catch {
    return true;
  }
}
