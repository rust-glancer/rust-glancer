import { setTimeout as delay } from "node:timers/promises";

/** Bound individual operations too: a polling deadline cannot interrupt a stalled read. */
export async function withTimeout<T>(
  operation: PromiseLike<T>,
  description: string,
  timeoutMs = 10_000,
): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`Timed out: ${description}`)), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

/** Wait for observable state; include the last value when it never becomes ready. */
export async function waitFor<T>(
  description: string,
  read: () => T | PromiseLike<T>,
  accepts: (value: T) => boolean,
  timeoutMs = 10_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  let last: T | undefined;
  while (Date.now() < deadline) {
    const value = await withTimeout<T>(
      Promise.resolve().then(read),
      description,
      Math.max(1, deadline - Date.now()),
    );
    last = value;
    if (accepts(value)) {
      return value;
    }
    await delay(25);
  }
  throw new Error(`Timed out: ${description}; last value: ${JSON.stringify(last)}`);
}
