# MangaVox cloud relay

The Cloudflare Worker behind MangaVox's **From anywhere** phone access, served at
`https://app.bhippi.com/mangavox`. It serves the small phone connector page (`web/`) and passes
end-to-end encrypted bytes between a PC and its paired phones. It never sees keys or content.
The protocol and threat model are in [native/crates/ui-phone/CLOUD.md](../native/crates/ui-phone/CLOUD.md).

## Run it locally

```bash
npm run dev
```

That serves `http://localhost:8787/mangavox`. Point a dev build of MangaVox at it with
`MANGAVOX_CLOUD_URL=http://localhost:8787/mangavox`, or run the headless PC from
`native/` with a throwaway data folder:

```bash
MANGAVOX_HOME=/tmp/mv-cloud MANGAVOX_CLOUD_URL=http://localhost:8787/mangavox cargo run -p ui-phone --example cloud_peer
```

Then pass the printed pairing link to the end-to-end test:

```bash
npm test -- '<pairing link>'
```

## Deploy

```bash
npm run deploy
```

This publishes the Worker on the `app.bhippi.com` custom domain (Cloudflare creates the DNS record).
Keep `app.bhippi.com` for this Worker only: the connector page is the phone's trust anchor, and
any other script on that origin could use its keys.
