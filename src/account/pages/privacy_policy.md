# Privacy Policy for eBIRForms

**Last updated:** November 25, 2026

This policy covers eBIRForms for macOS, Windows, Linux, iPhone, iPad and Android, version 1.0.0 and later, published by
Goldcoders Corp. It says what the app stores, what it sends, and to whom.

## In short

- Your taxpayer profiles and returns are stored on your own device.
- You need a Buwiz account to use the app. Goldcoders holds the account's email address, the TINs you register with
  it, the documents you send to verify a TIN (until that request is decided), and a record of each return you send.
- When you send a return, it travels through Buwiz's sending service on its way to the Bureau of Internal Revenue
  (BIR). That service opens the return in memory to check it and to encrypt it the way the BIR requires. It does not
  keep the return's contents after the send.
- A return is sent only when you press Queue.
- The app has no analytics, no advertising, no crash reporting and no usage tracking.

## 1. Your Buwiz account

The app asks you to sign in to a Buwiz account before it can be used. Sign-in happens in your browser on
`account.buwiz.com`, with a six-digit code sent to your email address. The app never sees a password; there is none.

**What Goldcoders holds about the account**

- The email address and an account id, and any staff action on the account (a ban, for example), with the staff
  member who took it.
- A record of each sign-in code you asked for: the email address, the times, the number of attempts, and the code
  only in scrambled form. These records are deleted after 30 days.
- Each device you have signed in on: the name the app gives it (such as "eBIRForms on macos"), a public key the app
  created on that device, and when it was registered and last used. A device you remove is marked as removed; its
  record stays.
- The sign-in sessions and tokens needed to keep you signed in.

We do not store your IP address. To measure whether the BIR's servers are reachable, we keep for 90 days a scrambled
code of the network a request came from and one of the account (not the address or the account itself, and both made
with a key that changes daily and is never stored), together with the app's name and version. They cannot be traced
back to your account once the day's key is gone.

**The TINs on your account.** A return can be sent only for a TIN that is registered and approved on your account.
For each TIN we hold: the TIN and branch code, the registered name and trade name, whether it is personal or
business, its status, who decided it and when, any note or reason our staff recorded (for a correction, a pause or a
removal), and the result of our automatic check (scores, not the text that was read). To approve a TIN we ask for
documents (for example the Certificate of Registration, form 2303, and a government ID). Documents are encrypted on
our server before they are stored. They are deleted when the request is decided or withdrawn; their file name,
type, size and a fingerprint stay as a record that they were provided, unless you delete the request first. Text we
read from a document to check it (the TIN, names, birth date, RDO) is deleted with the document. A TIN that was
once approved keeps its record after you remove it, because the records of returns sent for it refer to it.

**What the app keeps on your device:** the saved sign-in (tokens, your email address and account id), in the
operating system's credential store and in a file beside the app's database; and the device's key and id, in the
credential store, or in files beside the database where the system has none.

**When the app contacts our server (`api.buwiz.com`):** when you sign in or out, each time the app starts (to renew
the sign-in), and when you send a return. A send also renews the sign-in if it has expired, registers the device the
first time (its name and public key), and downloads two public documents: our signed list of trusted keys and
servers, and the sending service's public key. Preparing, checking and printing a return works without a connection to
our server once you have signed in.

**Practice accounts.** An administrator can mark an account as a practice account (the kind app-store reviewers
use). The app then only simulates sending: nothing from that account reaches the BIR or our sending service.

## 2. What is stored on your device

The app keeps its data in one folder:

| System | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/dev.goldcoders.bir/` (inside the app's container for the Mac App Store version) |
| Windows | `%LOCALAPPDATA%\dev.goldcoders.bir\` |
| Linux | `$XDG_DATA_HOME/dev.goldcoders.bir`, or `~/.local/share/dev.goldcoders.bir` |
| iPhone, iPad and Android | The app's own storage on the device |

It holds:

- **Taxpayer profiles:** name, TIN, RDO code, line of business, registered address, phone, email, taxpayer type and
  the other registration details you enter.
- **Returns:** drafts, sent copies, amendments, the signed receipt for each send made through Buwiz, and BIR
  confirmation emails the app has matched to them.
- **The saved sign-in,** and the device's key where the system has no credential store.
- **Settings and locks:** if you set a PIN, a recovery key or an authenticator, the app stores what it needs to check
  them (a hash for the PIN and the recovery key).
- **Inbox access, if you turn on receipt tracking:** the mailbox address and either an app password or the Google
  sign-in tokens.
- **Logs:** daily files kept for 7 days unless you change that. Passwords and tokens are removed from logs. TINs do
  appear in them.

**The database is a standard SQLite file and the app does not encrypt it.** Anyone who can read your user account's
files can read it, including a saved inbox password or Google token. Protect it the way you protect other private
files: a password on your user account and disk encryption (FileVault, BitLocker or LUKS).

To remove everything the app has stored on a device, sign out first (that clears the sign-in and the device's key
from the credential store), then delete the folder above.

On an iPhone, iPad or Android device there is no folder to delete: deleting the app removes what it stored.
Deleting the app does not remove the device from your Buwiz account: sign out first, or remove the device on the
account site.

## 3. Sending a return to the BIR

Nothing is sent until you press Queue on a return. The app first downloads our signed list of trusted keys and
servers, and seals nothing unless the sending service's key is on a list signed by the key built into the app. Then:

1. **The app asks our server for permission to send.** That request carries your sign-in, the device's id and a
   signature made with the device's key, the app's name and version and the operating system's name, the TIN, the form, the period, the email
   address the BIR should confirm to, a fingerprint (a hash) of the return, and a request id. Our server checks
   that the TIN can send on your account and that you are within the send limits, and answers with a signed ticket.
2. **The app seals the return on your device** for Buwiz's sending service, and hands it to our server. Our server
   stores it sealed and cannot open it; only the sending service holds the key that opens it.
3. **The sending service opens the return in memory,** checks that it matches the ticket (the same TIN, form and
   period), and encrypts it in the format the BIR requires. It writes the return to no disk, no database and no log,
   and its memory is never swapped to disk.
4. **Everything that goes to the BIR travels through connections your device opens.** The app connects only to the
   BIR servers named on our signed list and passes the bytes along without reading them, so the BIR sees your own
   network address, not ours. First the sending service asks the BIR's server where to upload; that request
   contains the first nine digits of the TIN and the form number, and uses HTTPS, falling back to plain HTTP if the
   BIR's server does not answer over HTTPS (those digits then cross your own network unencrypted). The upload
   itself is encrypted between our sending service and the BIR.
   The file's name contains the TIN and the confirmation email address, as the BIR requires. The BIR then emails
   its confirmation to that address.
5. **Our server gives the app a signed receipt.** The app checks its signature against the keys on our signed list
   before it marks the return as sent, and keeps the receipt with the return.

**What we keep about each send, and for how long**

- A record of the send: the account, the device, the TIN, the form, the period, the file name (it contains the TIN
  and the confirmation email address), fingerprints of the return and of the file sent to the BIR, the times, the
  outcome, an error message if it failed, the ticket from step 1, and a copy of the signed receipt. We keep these
  records for as long as the account exists, and delete them when the account is deleted.
- When a send is held because the BIR's servers were offline or busy, a record of that (the account, the TIN, the
  form and the period) is kept for 90 days, so that an outage on a deadline day can be shown. A signed proof of an
  outage names the TIN only in masked form.
- The sealed return itself is deleted from our database as soon as the send ends, whether it succeeded or failed. A
  return that was handed over but never sent is deleted within about 70 minutes.
- Our database provider keeps a history of changes for 6 hours, so the database can be restored after a fault.
  A record or a sealed return that was deleted can still exist in that history for up to 6 hours, and is then
  gone. (Read from the provider's settings on 2026-11-25.)
- Our server's logs record ids, not TINs or email addresses, and are kept for 30 days. They record which page or
  address was asked for, never what was typed after it.

**What does not go through us.** The app's own dry run (a simulation you can switch on; development builds use it
by default) and anything done on a practice account: these send nothing to the BIR and nothing about a return to us.

## 4. Receipt tracking (optional)

If you turn on receipt tracking for a taxpayer, the app reads that mailbox for the BIR's confirmation emails, so a
return can move to Confirmed and Paid on its own. You choose one of two ways:

- **Google sign-in.** You approve access in your browser. Google's permission for mail access over IMAP
  (`https://mail.google.com/`) is broad: it allows reading the whole mailbox. The app uses it only to search for
  messages from `ebirforms-noreply@bir.gov.ph`. It also reads the address of the account you connected.
- **Another mailbox over IMAP,** with an app password you enter.

The mailbox is read from your device. Nothing from it is sent to Goldcoders. Disconnecting removes the saved password
or tokens from your device; it does not change anything Google or your mail provider holds. Google handles the
sign-in under its own terms and privacy policy.

## 5. Who else handles this data

We use these companies to run the service. Each receives only what its job needs.

| Company | What it does for us | What it receives |
| --- | --- | --- |
| Cloudflare | Carries all traffic to our servers and protects sign-in from abuse; stores the encrypted TIN documents | Everything the app sends to our server passes through it, including the permission request in step 1 above. The sealed return stays sealed, and the upload to the BIR stays encrypted. For the sign-in check it receives your network address. |
| Neon | Our database (hosted in Singapore) | Everything listed in sections 1 and 3 as held by us |
| Amazon Web Services | The servers that run our service (Singapore) | The same data while it is being processed, and server logs |
| Resend | Sends the sign-in code | Your email address and the code. Resend keeps its own log of sent messages for its own retention period. |

The BIR receives the return you send and its file name. We do not sell personal data, and we do not share it for
advertising.

## 6. Update checks

The Mac App Store, Microsoft Store, App Store (iPhone and iPad) and Google Play versions never check for updates themselves; the
store delivers them. Versions downloaded from the app's website or its release page ask GitHub, where the releases
are published, whether a newer version exists, shortly after starting and every few hours. That request carries no
information about you beyond what any web request does, such as your IP address.

## 7. What the app does not do

- No analytics, advertising, behavioural tracking, crash reporting or remote logging.
- No access to your contacts, camera, microphone or location.
- No sale or sharing of personal data.

## 8. Deleting your account

Use Settings > Account > Delete account in the app, which opens `account.buwiz.com/delete`, or open that page
yourself. You sign in, ask for a code by email, and type it; that code is separate from the one you sign in with.
The page `https://ebirforms.buwiz.com/delete-account` explains the steps and needs no sign-in.

**What is deleted:** the account and its email address; your devices, sign-ins and tokens (the app is signed out);
the TINs on the account with their documents; the records of the returns you sent, with their tickets and receipts;
the records of sends held during a BIR outage; and the records of sign-in codes you asked for.

**What stays:** lines in our staff audit logs, which name a TIN only in masked form and the account only by its id;
the scrambled records described in section 1, for their 90 days; server logs, which carry ids only, for
their 30 days; and whatever our database provider's backups and our email provider's message log still hold for
their own limited time.

**When deletion is refused:** an account with a staff role must ask an administrator; an account that is named in
staff work on other accounts must write to support@buwiz.com; an account that has been banned must write to
support@buwiz.com from its address, and we delete it for you; and while a return is being sent, try again a few
minutes later.

Deleting the account does not delete what is stored on your device; sign out and delete the folder in section 2
for that. It does not withdraw returns the BIR has already received.

## 9. Children

eBIRForms is a tool for filing tax returns and is not directed at children.

## 10. Changes

When this policy changes, the date at the top changes with it. The history of this file is public in the app's
repository.

## 11. Contact

Write to support@buwiz.com. The app's website is https://ebirforms.buwiz.com.
