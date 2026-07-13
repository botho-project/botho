/**
 * Global `fetch` bound to the Workers global scope.
 *
 * workerd's `fetch` throws `TypeError: Illegal invocation: function called
 * with incorrect \`this\` reference` when invoked with any receiver other
 * than the global scope — which is exactly what happens when the bare
 * `fetch` reference is stored as a class field (`this.fetchImpl(...)` calls
 * it with the instance as `this`) or passed around unbound. Browsers
 * tolerate the same pattern and unit tests inject mocks, so the failure
 * only ever surfaces in production workerd.
 *
 * Every injectable `fetchImpl` parameter in this Worker must default to
 * `boundFetch`, never to bare `fetch`.
 */
export const boundFetch: typeof fetch = fetch.bind(globalThis)
