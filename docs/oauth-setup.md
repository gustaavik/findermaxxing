# Setting up OAuth for Google Drive, Dropbox and OneDrive

FinderMaxxing signs in to cloud drives with OAuth 2.0 + PKCE in the system browser (ADR 0023). Each provider needs
an app registered under **your** developer account; the IDs are compiled into the build. Nothing here is a
secret that protects anything: PKCE does. Still, keep the Google client secret out of public repos.

## 1. Google Drive

1. <https://console.cloud.google.com/> → create a project (e.g. "FinderMaxxing").
2. *APIs & Services → Library* → enable **Google Drive API**.
3. *APIs & Services → OAuth consent screen*:
   - user type **External**; app name, support email, logo, privacy policy URL;
   - scopes: add `https://www.googleapis.com/auth/drive` (a **restricted** scope);
   - test users: add the Google accounts you'll test with (up to 100 while unverified).
4. *Credentials → Create credentials → OAuth client ID* → type **Desktop app**. Note the client ID and secret.
   Loopback redirects (`http://127.0.0.1:<any port>`) are allowed for desktop clients by default.
5. Before public release: *Publish app* → verification. The restricted `drive` scope requires a **CASA security
   assessment** by a Google-approved assessor (yearly, paid). Until then, sign-in shows "unverified app" and
   only test users can sign in.

## 2. Dropbox

1. <https://www.dropbox.com/developers/apps> → *Create app* → **Scoped access** → **Full Dropbox**.
2. *Permissions*: tick `account_info.read`, `files.metadata.read`, `files.content.read`, `files.content.write`
   → *Submit*.
3. *Settings*:
   - *OAuth 2 → Redirect URIs*: add exactly `http://localhost:53682/`;
   - *Allow public clients (Implicit Grant & PKCE)*: **Allow**;
   - note the **App key** (no secret is used).
4. Before release: *Apply for production* (needed beyond 500 users).

## 3. OneDrive (Microsoft Graph)

1. <https://entra.microsoft.com/> → *Applications → App registrations → New registration*.
   - Supported accounts: **Accounts in any organizational directory and personal Microsoft accounts**.
   - Redirect URI: platform **Public client/native (mobile & desktop)**, URI `http://localhost`. Any port is
     accepted for it.
2. *Authentication*: *Allow public client flows* → **Yes**.
3. *API permissions*: Microsoft Graph, **delegated**: `Files.ReadWrite.All`, `User.Read`, `offline_access`.
4. Note the **Application (client) ID**. Work/school tenants may need an admin to consent to `Files.ReadWrite.All`.
5. Before release: *Branding & properties* → set a verified publisher domain (otherwise users see "unverified").

## 4. Building with the IDs

The IDs are read at **compile time**:

```sh
export FM_GOOGLE_CLIENT_ID=…apps.googleusercontent.com
export FM_GOOGLE_CLIENT_SECRET=…
export FM_DROPBOX_APP_KEY=…
export FM_ONEDRIVE_CLIENT_ID=…
cargo build -p app --release
```

Debug builds also read them at run time (`cargo run -p app`), so you can try without rebuilding. CI should keep
them as repository secrets.

## 5. Live tests

With the variables above set, each adapter's ignored live test signs in through your browser once and prints a
refresh token. Export it to skip the browser next time. Then it creates a throwaway folder, writes, lists, reads
and deletes it:

```sh
cargo test -p adapter-gdrive   --test drive_contract    live_round_trip -- --ignored --nocapture
cargo test -p adapter-dropbox  --test dropbox_contract  live_round_trip -- --ignored --nocapture
cargo test -p adapter-onedrive --test onedrive_contract live_round_trip -- --ignored --nocapture
# then e.g. export FM_LIVE_GOOGLE_REFRESH_TOKEN=… (and _DROPBOX_, _ONEDRIVE_)
```

## 6. Trying the app without any accounts

```sh
cargo run -p mock-cloud --bin mock-cloud-server        # prints FM_DEV_CLOUD_MOCK=http://127.0.0.1:<port>
FM_DEV_CLOUD_MOCK=http://127.0.0.1:<port> cargo run -p app
```

*Add Cloud Storage…* → Google Drive / Dropbox / OneDrive then "signs in" against the local doubles (debug builds only).
