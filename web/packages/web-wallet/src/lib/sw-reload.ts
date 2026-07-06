/**
 * Service-worker auto-update reload policy.
 *
 * `main.tsx` reloads the page once a freshly deployed service worker takes
 * control (the `controllerchange` event) so visitors get the new build within a
 * single navigation. That reload is normally harmless — but the `/pay` and
 * `/claim` pages read a one-time secret/address from the URL FRAGMENT and then
 * immediately STRIP it from the address bar for privacy (#589). By the time the
 * SW activates (a few seconds after first paint) the fragment is already gone,
 * so a reload would land on a fragment-less URL and render "not found",
 * destroying a valid payment/claim link (issue #654).
 *
 * The fragment is intentionally NOT persisted anywhere (that is the whole point
 * of #589), so it cannot be recovered after a reload. The correct fix is to NOT
 * reload these fragment-consuming pages: the app is already loaded and fully
 * functional, and the visitor picks up the new build on their next navigation.
 */

/**
 * Routes that consume a one-time URL fragment on mount and must not be reloaded
 * by the service-worker auto-update, or the (already-stripped) fragment is lost.
 */
export const FRAGMENT_CONSUMING_PATHS = ['/pay', '/claim'] as const

/**
 * Whether a `controllerchange` auto-update reload is safe for the given path.
 * Returns `false` for the fragment-consuming link pages (`/pay`, `/claim`).
 */
export function shouldReloadOnControllerChange(pathname: string): boolean {
  return !FRAGMENT_CONSUMING_PATHS.includes(pathname as (typeof FRAGMENT_CONSUMING_PATHS)[number])
}
