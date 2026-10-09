# Using FmS on Multiple Devices

FmS runs on your desktop(s) and phone at the same time and keeps your datasets, practice progress, and chat in sync between them. This page explains the model (Hub and Follower), how to set it up once, and what to do day to day.

Everything related to multi-device setup lives under **Tools** in the sidebar:

- **Devices & Hub** — role, cluster identity, pairing, and the device list.
- **Datasets Sync** — pulling datasets down and pushing edits back up.
- **Cross-Device Chat** — messaging between your own devices.

---

## Hub and Follower

Every **workspace** plays one of two roles:

- A **Hub** stores the authoritative copy of all your datasets. It serves them over the network to your other devices. There is only one Hub at a time — it is the source of truth every other device syncs against.
- A **Follower** (a phone, or a second PC) downloads the datasets it wants from the Hub and syncs its changes back up when you edit something locally.

A few rules worth knowing up front:

- Roles belong to a **workspace**, not to the whole app. The same machine can host one workspace that is a Hub and another that is a Follower.
- A **phone is always a Follower** — it can never serve datasets to other devices. The Hub option only shows up on desktop.
- Every desktop automatically starts a small network service (on port **35711**) as soon as the app is open. That is what makes it discoverable to other devices — you don't need to turn anything on manually.

### Recommended setup

1. On your main PC (the one that's usually on), create or select a workspace and set its role to **Hub**. Keep all your real datasets there.
2. Treat every other PC and phone as a **Follower** of that Hub.

---

## Setting up the Hub

1. Open **Tools → Devices & Hub** on the machine you want to be the Hub.
2. Under **Current Workspace Status**, find the **Sync Role** dropdown (desktop only) and choose **Hub (authoritative copy)**.
3. The page now shows a **Cluster** ID (a short code, e.g. `a1b2c3d4`). This ID is generated once, the first time a workspace becomes a Hub, and every follower that pairs with it adopts the same ID so devices never talk to the wrong Hub.
4. Your section header now reads "**hub · serving datasets to paired devices**" on the Datasets Sync page, and a **Devices paired to me** list appears here — this is where you approve or revoke follower devices later.

You do not need to open any network ports yourself or start a server — the app does this automatically whenever it's running.

---

## Connecting a Follower (phone or second PC)

Do this on the device that should sync *from* the Hub.

1. Open **Tools → Devices & Hub**.
2. Under **Hub**, pick the Hub machine:
   - **Scan** searches your local network (and Tailscale, if you use it) for other machines running FmS and lists them by hostname and address.
   - **Manual** lets you type the address directly, e.g. `http://192.168.1.20:35711`, if scanning doesn't find the machine (some Wi-Fi networks block the discovery broadcasts FmS uses).
   - Selecting an address saves it and probes the Hub once so this device adopts the Hub's cluster ID (see the **Cluster** line under Current Workspace Status).
3. Click **Pair**. This is a one-time step, like pairing Bluetooth headphones: a confirmation dialog pops up on the **Hub machine**, showing a short code (for example `4f2a1b`) and the name of the device asking to connect, plus which user account it will sync progress as.
4. Check that the code on the Hub matches the code shown on your device's Devices & Hub page while it says "Waiting for the PC owner to confirm," then click **Allow** on the Hub.
5. The follower now shows **Paired** next to the Hub address. From here on, syncing happens without another confirmation.

> **Note:** Pairing is only needed on a normal local network (LAN/WLAN). Two other cases skip it entirely — see the **Trust zones** section below.

### If the Hub is ever replaced or you want to start over

Use **Forget hub / re-pair** on the follower. This clears the saved cluster ID and Hub address (your local datasets and progress are kept untouched) so you can connect to a different Hub, or re-pair cleanly with the same one.

### If a pairing request was denied

The Hub owner can block a device by mistake. On the follower, click **New identity** to generate a fresh device ID, then press **Pair** again — this sends a new request that shows up as a new dialog on the Hub.

---

## Syncing datasets day to day

Once paired, go to **Tools → Datasets Sync** on the follower.

- The page lists every dataset the Hub currently offers, grouped into tabs: **Dictation**, **Cards**, **Books**, **Read**.
- Each dataset row shows its sync state:
  - **Not downloaded** — you've never pulled a copy. Click **Download** to get the Hub's full copy of it (this replaces anything you had locally).
  - **Downloaded** — you have a local copy. If the Hub has newer changes, the row will say "Hub has newer changes — run 'Sync now' to pull them."
  - **Resync due** — the local copy is out of date in a way that needs a full fresh download; click **Download** to adopt the Hub's copy again.
  - **Removed on hub** — the Hub no longer has this dataset. Use **Remove local copy** under the "Removed on hub" section if you want to free up space.
- **Sync now** runs one full sync round: it pushes any of your local edits up to the Hub, pulls down whatever changed on the Hub since last time, and updates your per-user progress (see below). It's safe to run this often — unchanged data isn't re-transferred.
- **Refresh** just re-reads the current status without syncing anything.

### What "sync" covers

- **Dataset content** (media files, subtitles, cues, cards, books, read-aloud recordings) moves between the Hub and followers.
- **Your personal progress** — dictation history and XP — is stored centrally on the Hub and shared with every follower that pairs under the same user account (the "Syncs as" column on the Hub's device list shows which account a device writes progress as).
- **Heavy structural operations are follower-only-in-one-direction**: creating, deleting, or moving whole datasets, and downloading/importing new media should be done on the Hub itself. A follower's edits to existing content (text changes, reviews, answers) are what get pushed back.

### Conflicts

If you edit the same item on two devices, the Hub keeps whichever version has the later edit time. A pending local edit that hasn't synced yet is never silently overwritten by something pulled down — the sync waits until your own change has been pushed first.

---

## Cross-device chat

**Tools → Cross-Device Chat** is one shared conversation between your own devices (e.g. send yourself a file from your PC to your phone). All chat messages and attachments are stored only on the Hub — a follower never keeps its own separate copy of the history.

- You can send messages and files while the Hub is reachable. If the Hub is offline, the Devices & Hub page will show a count of pending chat messages ("`N` chat message(s) pending") and the send will simply fail until the Hub comes back online — retry then.
- Attachments are downloaded on demand to the follower's own storage when you open them; they don't take up space until you view or save them.

---

## Trust zones

FmS decides how much to trust a connecting device based on where the request comes from:

| Where the request comes from | What's required |
|---|---|
| The same machine (localhost) | Nothing — always trusted, no pairing needed |
| A Tailscale network address | Nothing — Tailscale's own access control is trusted, no pairing needed |
| Anywhere else on your LAN/WLAN | A one-time pairing approval on the Hub, as described above |

This means a phone on your home Wi-Fi needs to pair once, but automations or scripts running on the Hub machine itself, or reaching it over a Tailnet from another location, don't need any pairing step.

---

## Troubleshooting

- **"Not connected to a hub."** — Go to Devices & Hub and pick or type a Hub address first; Datasets Sync won't show anything until it does.
- **"Cannot reach the PC at …"** — The Hub machine may be off, asleep, or running a different app version. Confirm FmS is open there, and that the address (including port `35711`) is correct.
- **"This device is not paired with the source PC yet."** — Click **Pair** (Datasets Sync links you straight to it) and have someone approve the request on the Hub machine.
- **A cluster binding error ("belongs to cluster …")** — This device was previously paired with a different Hub. Use **Forget hub / re-pair** to clear the old binding before connecting to this one.
- **Sync seems stuck or slow after a Hub change** — Press **Refresh** on Datasets Sync; if a dataset shows **Resync due**, use **Download** to force a fresh full copy from the Hub.
- **You revoked a device but it still appears to connect** — Revoking on the Hub removes its access immediately; the device itself needs to re-pair (or use **New identity**) to get access back.

---

## Managing paired devices (Hub side)

On the Hub machine's **Devices & Hub** page, the **Devices paired to me** table lists every follower that has ever requested access:

- **Device / Code** — the device's name and the short fingerprint both sides saw during pairing.
- **Status** — `approved` or a denied entry.
- **Syncs as** — which user account this device writes dictation progress and XP under.
- **Last seen** — when this device last successfully synced.
- **Revoke / Remove** — revoke an approved device (blocks future syncs), or remove a denied entry from the list.
