//! The privacy policy of the eBIRForms app (`/privacy` on the filing site),
//! the address the app stores list. The text is the app's own
//! `PRIVACY_POLICY.md` (hexuria/buwiz-forms), copied beside this file as
//! `privacy_policy.md`; a test checks that the page says exactly what that
//! copy says, so a new version of the policy is a new copy and then this
//! page to match. Styles are in `src/sites/home.css`.

use http::header;

use super::home::footer;
use super::*;
use crate::sites::canonical::Site;

pub(super) const TITLE: &str = "Privacy Policy for eBIRForms · Buwiz";
pub(super) const DESCRIPTION: &str =
    "What the eBIRForms app for macOS, Windows and Linux stores, what it sends, and to whom.";

#[page("/privacy")]
pub async fn privacy_page(cx: &Cx) -> Result<impl View> {
    let host = headers(cx).get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    // The filing site's only: buwiz.com is the Bot site's, and the account
    // and API hosts have no public pages.
    if Site::for_host(host) != Site::Forms {
        return Err(topcoat::router::error::not_found().into());
    }
    Ok(view! {
        <main class="home landing">
            <section class="section policy-page">
                <article class="policy">
                    <h1>"Privacy Policy for eBIRForms"</h1>
                    <p class="policy-updated"><strong>"Last updated:"</strong>" November 24, 2026"</p>
                    <p>
                        "This policy covers eBIRForms for macOS, Windows and Linux, version 0.2.0 and later, published by Goldcoders Corp. It says what the app stores, what it sends, and to whom."
                    </p>

                    <h2>"In short"</h2>
                    <ul>
                        <li>"Your taxpayer profiles and returns are stored on your computer. They are not sent to Goldcoders."</li>
                        <li>"You need a Buwiz account to use the app. Goldcoders receives the account's email address and account id, and nothing about your taxes."</li>
                        <li>"A return is sent to the Bureau of Internal Revenue (BIR) only when you press Submit and confirm. It goes from your computer straight to the BIR."</li>
                        <li>"The app has no analytics, no advertising, no crash reporting and no usage tracking."</li>
                    </ul>

                    <h2>"1. Your Buwiz account"</h2>
                    <p>
                        "The app asks you to sign in to a Buwiz account before it can be used. Sign-in happens in your browser on "
                        <code>"account.buwiz.com"</code>
                        ", not inside the app."
                    </p>
                    <ul>
                        <li><strong>"What Goldcoders holds:"</strong>" the email address of the account and an account id."</li>
                        <li><strong>"What the app keeps on your computer:"</strong>" a sign-in token, in the operating system's credential store and in a file beside the app's database."</li>
                        <li>
                            <strong>"When the app contacts our server ("<code>"api.buwiz.com"</code>"):"</strong>
                            " when you sign in or out, and each time the app starts, to renew the sign-in. After you have signed in, the app keeps working without a connection to our server."
                        </li>
                        <li><strong>"What it is used for:"</strong>" signing you in. It is not used for advertising and is not sold or shared."</li>
                        <li><strong>"Practice accounts:"</strong>" our server can mark an account as a practice account (the one app-store reviewers use). The app then only simulates sending a return; nothing from that account reaches the BIR."</li>
                    </ul>
                    <p>
                        "To delete your Buwiz account and the email address held with it, use Settings > Account > Delete account in the app (from version 0.2.1), or write to "
                        <a href="mailto:support@buwiz.com">"support@buwiz.com"</a>
                        " from that address."
                    </p>

                    <h2>"2. What is stored on your computer"</h2>
                    <p>"The app keeps its data in one folder:"</p>
                    <div class="policy-table">
                        <table>
                            <thead>
                                <tr><th>"System"</th><th>"Folder"</th></tr>
                            </thead>
                            <tbody>
                                <tr>
                                    <td>"macOS"</td>
                                    <td><code>"~/Library/Application Support/dev.goldcoders.bir/"</code>" (inside the app's container for the Mac App Store version)"</td>
                                </tr>
                                <tr>
                                    <td>"Windows"</td>
                                    <td><code>"%LOCALAPPDATA%\\dev.goldcoders.bir\\"</code></td>
                                </tr>
                                <tr>
                                    <td>"Linux"</td>
                                    <td><code>"$XDG_DATA_HOME/dev.goldcoders.bir"</code>", or "<code>"~/.local/share/dev.goldcoders.bir"</code></td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                    <p>"It holds:"</p>
                    <ul>
                        <li><strong>"Taxpayer profiles:"</strong>" name, TIN, RDO code, line of business, registered address, phone, email, taxpayer type and the other registration details you enter."</li>
                        <li><strong>"Returns:"</strong>" drafts, filed copies, amendments, and BIR receipts the app has matched to them."</li>
                        <li><strong>"Settings and locks:"</strong>" if you set a PIN, a recovery key or an authenticator, the app stores what it needs to check them (a hash for the PIN and the recovery key)."</li>
                        <li><strong>"Inbox access, if you turn on receipt tracking:"</strong>" the mailbox address and either an app password or the Google sign-in tokens."</li>
                        <li><strong>"Logs:"</strong>" daily files kept for 7 days unless you change that. Passwords and tokens are removed from logs. TINs do appear in them."</li>
                    </ul>
                    <p>
                        <strong>"The database is a standard SQLite file and the app does not encrypt it."</strong>
                        " Anyone who can read your user account's files can read it, including a saved inbox password or Google token. Protect it the way you protect other private files: a password on your user account and disk encryption (FileVault, BitLocker or LUKS)."
                    </p>
                    <p>"Deleting the folder above removes everything the app has stored on that computer."</p>

                    <h2>"3. What is sent to the BIR"</h2>
                    <p>"Nothing is sent to the BIR until you press Submit on a return and confirm. Then, from your computer:"</p>
                    <ol>
                        <li>"The app asks the BIR's server where to upload. That request contains the first nine digits of the TIN and the form number. It uses HTTPS, and falls back to plain HTTP if the BIR's server does not answer over HTTPS."</li>
                        <li>"The app uploads the return over SFTP to the server the BIR names, as an encrypted file in the BIR's own format. The file's name contains the TIN and the taxpayer's email address, as the BIR requires."</li>
                    </ol>
                    <p>"Goldcoders does not receive, relay or keep a copy of that traffic."</p>

                    <h2>"4. Receipt tracking (optional)"</h2>
                    <p>
                        "If you turn on receipt tracking for a taxpayer, the app reads that mailbox for the BIR's confirmation emails, so a return can move to Confirmed and Paid on its own. You choose one of two ways:"
                    </p>
                    <ul>
                        <li>
                            <strong>"Google sign-in."</strong>
                            " You approve access in your browser. Google's permission for mail access over IMAP ("
                            <code>"https://mail.google.com/"</code>
                            ") is broad: it allows reading the whole mailbox. The app uses it only to search for messages from "
                            <code>"ebirforms-noreply@bir.gov.ph"</code>
                            ". It also reads the address of the account you connected."
                        </li>
                        <li><strong>"Another mailbox over IMAP,"</strong>" with an app password you enter."</li>
                    </ul>
                    <p>
                        "The mailbox is read from your computer. Nothing from it is sent to Goldcoders. Disconnecting removes the saved password or tokens from your computer; it does not change anything Google or your mail provider holds. Google handles the sign-in under its own terms and privacy policy."
                    </p>

                    <h2>"5. Update checks"</h2>
                    <p>
                        "The Mac App Store and Microsoft Store versions never check for updates themselves; the store delivers them. Versions downloaded from GitHub ask GitHub, where the app's releases are published, whether a newer version exists, shortly after starting and every few hours. That request carries no information about you beyond what any web request does, such as your IP address. The Microsoft Store version behaves this way from version 0.2.1."
                    </p>

                    <h2>"6. What the app does not do"</h2>
                    <ul>
                        <li>"No analytics, advertising, behavioural tracking, crash reporting or remote logging."</li>
                        <li>"No access to your contacts, camera, microphone or location."</li>
                        <li>"No sale or sharing of personal data."</li>
                    </ul>

                    <h2>"7. Children"</h2>
                    <p>"eBIRForms is a tool for filing tax returns and is not directed at children."</p>

                    <h2>"8. Changes"</h2>
                    <p>
                        "When this policy changes, the date at the top changes with it. The history of this file is public in the app's repository."
                    </p>

                    <h2>"9. Contact"</h2>
                    <p>
                        "Write to "
                        <a href="mailto:support@buwiz.com">"support@buwiz.com"</a>
                        ". The app's website is "
                        <a href="https://ebirforms.buwiz.com">"https://ebirforms.buwiz.com"</a>
                        "."
                    </p>
                </article>
            </section>
            footer()
        </main>
    })
}
