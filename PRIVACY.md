# Privacy

Rustroom is a photo editor that runs on your computer. **It collects nothing about you and sends nothing to its
authors** — there is no account, no analytics, no crash reporting and no advertising.

Your photos, edits, catalog, keywords, people's names, locations and settings stay on your computer, in Rustroom's own
folders and the files beside your photos. Rustroom goes online only for the things below, each of which you can see
coming and most of which you start yourself.

## What Rustroom sends, and where

| When | To | What it sends |
|---|---|---|
| You connect an **Immich** server (Settings → Immich) | the server you entered — your own | what Immich needs to show, download and upload your photos: your API keys, and the photos, ratings, favourites and albums you choose to send |
| You open the **Map** | the tile server in Settings → Behaviour (OpenStreetMap's by default) | which map tiles to draw — that is, the area you are looking at |
| You press Enter in the Map's **location field** | OpenStreetMap's Nominatim | the place name you typed |
| You first use an **AI feature** (masks, depth, denoise, removal, finding people) | GitHub or Hugging Face, where each model is published | a request to download that model; your photos are never sent — the AI runs on your computer |
| Rustroom starts, **at most once a day** | GitHub | a request for the newest Rustroom version number; turn it off in Settings → Behaviour → Check for new versions. The Microsoft Store version does not ask — the Store updates it |

Like any connection, each of these shows the server your IP address; the map, location and version requests also
name Rustroom and its version. What each server does with that is governed by its own policy:
[OpenStreetMap](https://osmfoundation.org/wiki/Privacy_Policy),
[GitHub](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement),
[Hugging Face](https://huggingface.co/privacy), and your Immich server's operator.

## Your Immich keys

The keys you give Rustroom for Immich are kept in your operating system's own credential store (the Windows
Credential Manager, the macOS keychain, or the Secret Service on Linux). Only if that store refuses them are they kept
in Rustroom's settings file in your user folder, until the store takes them.

## Changes and questions

Rustroom is open source, so all of the above can be checked in its code. If this policy changes, the change is in
this file's history. Questions: [github.com/wladiKrin/rustroom/issues](https://github.com/wladiKrin/rustroom/issues).
