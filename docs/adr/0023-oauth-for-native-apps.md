# ADR 0023 — OAuth 2.0 for native apps, and a shared infrastructure tier

**Status:** accepted (M5b)

## Context
Google Drive, Dropbox and OneDrive only accept OAuth. A desktop app can't keep a client secret, must never see the
user's password, and has to keep working for months without asking again.

## Decision
- **RFC 8252 (OAuth for native apps):**
  - the authorization code flow with PKCE (S256) in the system browser;
  - the redirect goes to a one-shot HTTP listener on the loopback interface (any free port for Google and
    Microsoft; Dropbox needs an exact URI, `http://localhost:53682/`);
  - a random `state` binds the answer to the attempt;
  - sign-in is cancellable from the dialog and gives up after 10 minutes.
- **Tokens:**
  - the refresh token goes to the OS keychain (`CredentialKey::drive_token`), never to disk;
  - access tokens live only in memory and are refreshed a minute before they expire, or after a 401;
  - rotated refresh tokens (Microsoft) are saved again at once.
  - A revoked grant (`invalid_grant`) becomes `PermissionDenied`: "sign in again".
- **`ApiClient`** sends with the bearer token, retries once after a refresh, and waits out 429/503 with the
  provider's `Retry-After` (at most 3 times, 30 s each).
- **`UrlOpener` port:** the browser is opened by `adapter-local-fs` (`open`/`xdg-open`, web URLs only).
- **A new layer, `infra-*`:** `infra-oauth` holds this code once for all three providers. Adapters may depend on
  `infra-*`; the core and presentation may not, and adapters still may not depend on each other. `arch-tests`
  enforces all of it.
- **Client IDs are build settings:** `FM_GOOGLE_CLIENT_ID`, `FM_GOOGLE_CLIENT_SECRET`, `FM_DROPBOX_APP_KEY`,
  `FM_ONEDRIVE_CLIENT_ID` (see `oauth-setup.md`). A build without one says so when the user tries that provider.
  Google's desktop "secret" isn't secret (RFC 8252 §8.5); PKCE is what protects the flow.
- **Development hook (debug builds only):** `FM_DEV_CLOUD_MOCK=<url>` points all three providers at
  `mock-cloud-server` and replaces the browser with a headless one. Release builds ignore it.

## Consequences
- Proven against `mock-cloud`'s authorization server: PKCE verification, a denied consent, cancellation,
  expiry and refresh, rotation, rate limits, revoked grants (`infra-oauth/tests/oauth_flow.rs`) and the RFC 7636
  test vector.
- Removing an account forgets the token here; revoking the grant at the provider is the user's step, and the
  dialog says so.
