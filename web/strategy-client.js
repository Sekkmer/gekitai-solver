/** Request/reply boundary for the long-lived, read-only strategy worker. */
export class StrategyClient {
  #worker;
  #pending = new Map();
  #nextId = 0;

  constructor(onProgress) {
    this.#worker = new Worker(
      new URL('./strategy-worker.js', import.meta.url),
      { type: 'module' },
    );
    this.#worker.onmessage = ({ data }) => {
      const pending = this.#pending.get(data.id);
      if (!pending) return;
      if (data.progress) {
        onProgress(data.progress);
        return;
      }
      this.#pending.delete(data.id);
      if (data.error) pending.reject(Error(data.error));
      else pending.resolve(data);
    };
    this.#worker.onerror = () => {
      for (const pending of this.#pending.values()) {
        pending.reject(
          Error('Strategy worker could not start. Reload the page to retry.'),
        );
      }
      this.#pending.clear();
    };
  }

  request(type, board) {
    const id = ++this.#nextId;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#worker.postMessage({ id, type, board });
    });
  }
}
