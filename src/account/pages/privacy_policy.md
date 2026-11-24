# Privacy Policy for eBIRForms

**Last updated:** November 24, 2026

This policy covers eBIRForms for macOS, Windows and Linux, version 0.2.0 and later, published by Goldcoders Corp. It
says what the app stores, what it sends, and to whom.

## In short

- Your taxpayer profiles and returns are stored on your computer. They are not sent to Goldcoders.
- You need a Buwiz account to use the app. Goldcoders receives the account's email address and account id, and nothing
  about your taxes.
- A return is sent to the Bureau of Internal Revenue (BIR) only when you press Submit and confirm. It goes from your
  computer straight to the BIR.
- The app has no analytics, no advertising, no crash reporting and no usage tracking.

## 1. Your Buwiz account

The app asks you to sign in to a Buwiz account before it can be used. Sign-in happens in your browser on
`account.buwiz.com`, not inside the app.

- **What Goldcoders holds:** the email address of the account and an account id.
- **What the app keeps on your computer:** a sign-in token, in the operating system's credential store and in a file
  beside the app's database.
- **When the app contacts our server (`api.buwiz.com`):** when you sign in or out, and each time the app starts, to
  renew the sign-in. After you have signed in, the app keeps working without a connection to our server.
- **What it is used for:** signing you in. It is not used for advertising and is not sold or shared.

To delete your Buwiz account and the email address held with it, use Settings > Account > Delete account in the app
(from version 0.2.1), or write to support@goldcoders.dev from that address.

## 2. What is stored on your computer

The app keeps its data in one folder:

| System | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/dev.goldcoders.bir/` (inside the app's container for the Mac App Store version) |
| Windows | `%LOCALAPPDATA%\dev.goldcoders.bir\` |
| Linux | `$XDG_DATA_HOME/dev.goldcoders.bir`, or `~/.local/share/dev.goldcoders.bir` |

It holds:

- **Taxpayer profiles:** name, TIN, RDO code, line of business, registered address, phone, email, taxpayer type and
  the other registration details you enter.
- **Returns:** drafts, filed copies, amendments, and BIR receipts the app has matched to them.
- **Settings and locks:** if you set a PIN, a recovery key or an authenticator, the app stores what it needs to check
  them (a hash for the PIN and the recovery key).
- **Inbox access, if you turn on receipt tracking:** the mailbox address and either an app password or the Google
  sign-in tokens.
- **Logs:** daily files kept for 7 days unless you change that. Passwords and tokens are removed from logs. TINs do
  appear in them.

**The database is a standard SQLite file and the app does not encrypt it.** Anyone who can read your user account's
files can read it, including a saved inbox password or Google token. Protect it the way you protect other private
files: a password on your user account and disk encryption (FileVault, BitLocker or LUKS).

Deleting the folder above removes everything the app has stored on that computer.

## 3. What is sent to the BIR

Nothing is sent to the BIR until you press Submit on a return and confirm. Then, from your computer:

1. The app asks the BIR's server where to upload. That request contains the first nine digits of the TIN and the form
   number. It uses HTTPS, and falls back to plain HTTP if the BIR's server does not answer over HTTPS.
2. The app uploads the return over SFTP to the server the BIR names, as an encrypted file in the BIR's own format. The
   file's name contains the TIN and the taxpayer's email address, as the BIR requires.

Goldcoders does not receive, relay or keep a copy of that traffic.

## 4. Receipt tracking (optional)

If you turn on receipt tracking for a taxpayer, the app reads that mailbox for the BIR's confirmation emails, so a
return can move to Confirmed and Paid on its own. You choose one of two ways:

- **Google sign-in.** You approve access in your browser. Google's permission for mail access over IMAP
  (`https://mail.google.com/`) is broad: it allows reading the whole mailbox. The app uses it only to search for
  messages from `ebirforms-noreply@bir.gov.ph`. It also reads the address of the account you connected.
- **Another mailbox over IMAP,** with an app password you enter.

The mailbox is read from your computer. Nothing from it is sent to Goldcoders. Disconnecting removes the saved
password or tokens from your computer; it does not change anything Google or your mail provider holds. Google handles
the sign-in under its own terms and privacy policy.

## 5. Update checks

The Mac App Store and Microsoft Store versions never check for updates themselves; the store delivers them. Versions
downloaded from GitHub ask GitHub (`github.com/hexuria/buwiz-forms`) whether a newer version exists, shortly after
starting and every few hours. That request carries no information about you beyond what any web request does, such as
your IP address. The Microsoft Store version behaves this way from version 0.2.1.

## 6. What the app does not do

- No analytics, advertising, behavioural tracking, crash reporting or remote logging.
- No access to your contacts, camera, microphone or location.
- No sale or sharing of personal data.

## 7. Children

eBIRForms is a tool for filing tax returns and is not directed at children.

## 8. Changes

When this policy changes, the date at the top changes with it. The history of this file is public in the app's
repository.

## 9. Contact

Write to support@goldcoders.dev, or open an issue at https://github.com/hexuria/buwiz-forms/issues.
