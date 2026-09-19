/**
 * Base URL for the app's own internal assets, served by the Rust side through
 * the custom `palace` scheme.
 *
 * Tauri exposes a registered custom scheme differently depending on the
 * platform (confirmed in the vendored crate, `tauri/src/protocol/isolation.rs`):
 *
 *   - macOS / iOS / Linux:  palace://localhost/<path>
 *   - Windows / Android:    http://palace.localhost/<path>
 *
 * WebView2 (Windows) cannot resolve a `<scheme>://localhost` authority the way
 * WebKit / WebKitGTK do, so Tauri rewrites the URL into a plain HTTP request
 * with the scheme folded into the host name. Hardcoding `palace://localhost/...`
 * therefore works everywhere except Windows, where every fetch fails.
 */
const usesLocalhostSubdomain = (): boolean =>
  typeof navigator !== "undefined" && /Windows|Android/i.test(navigator.userAgent);

export const internalBaseUrl = (): string =>
  usesLocalhostSubdomain() ? "http://palace.localhost" : "palace://localhost";
