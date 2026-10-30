//! The person's own pages: My TINs, adding a TIN, a claim's files, and
//! fixing a claim staff sent back. The add-a-TIN form is one form in three steps: with
//! JavaScript (`src/account.js`) it shows one step at a time; without, the
//! steps stack and the form still posts. Which papers show for a personal
//! or a business TIN follows the chosen radio button in CSS (`:has()`).

use super::*;

const ORUS: &str = "https://orus.bir.gov.ph/";

#[page("/tins")]
pub async fn tins_page(cx: &Cx) -> Result<impl View> {
    let account = signed_in(cx).await?;
    let mut claims = claims::for_account(&state(cx).db, account.id).await?;
    let devices = devices::for_account(&state(cx).db, account.id).await?;
    // Newest first. A rejected or revoked claim stays until the TIN is added
    // again; a removed TIN is gone from here.
    let mut seen = Vec::new();
    claims.retain(|claim| {
        let tin = (claim.tin_root.clone(), claim.branch_code.clone());
        let keep = match claim.status.as_str() {
            "withdrawn" | "removed" => false,
            "rejected" | "revoked" => !seen.contains(&tin),
            _ => true,
        };
        seen.push(tin);
        keep
    });
    Ok(view! {
        <main class="home page">
            <div class="page-in">
                <div class="page-head">
                    <div>
                        <h1>"My TINs"</h1>
                        <p class="page-lead">"Prepare returns for any TIN here. Sending them to the BIR opens once a TIN is active."</p>
                    </div>
                    <a class="btn btn-solid" href="/tins/new">icon(name: "add") "Add a TIN"</a>
                </div>
                page_notice(notice: notice(cx))
                <div class="tin-grid">
                    #[key(claim.id.as_u128())]
                    for claim in claims {
                        tin_card(claim: claim)
                    }
                    <a class="tin-add" href="/tins/new">
                        <span class="tin-add-icon">icon(name: "add")</span>
                        "Add another TIN"
                    </a>
                </div>
                <section class="devices" aria-labelledby="devices-h">
                    <h2 id="devices-h">"My devices"</h2>
                    <p class="page-lead">"Computers and phones that can send returns for you. Remove one you no longer use or don't recognise; it is signed out and can't send until it signs in again."</p>
                    if devices.is_empty() {
                        <p class="device-empty">"No devices yet. One appears here the first time you send a return from the Buwiz app."</p>
                    } else {
                        <ul class="device-list">
                            #[key(device.id.as_u128())]
                            for device in devices {
                                device_row(device: device)
                            }
                        </ul>
                    }
                </section>
            </div>
        </main>
    })
}

/// A device that can send returns, with a button to remove it.
#[component]
async fn device_row(device: devices::DeviceView) -> Result<impl View> {
    let revoke = format!("/devices/{}/revoke", device.id);
    let app = oauth::client_name(&device.client_id).unwrap_or("Buwiz");
    let used = device.last_used_at.map(short_date).unwrap_or_else(|| "never".to_owned());
    Ok(view! {
        <li class="device-row">
            <span class="device-icon">icon(name: "devices")</span>
            <span class="device-text">
                <strong>(device.name.clone())</strong>
                <span>(format!("{app} · added {} · last used {used}", short_date(device.created_at)))</span>
            </span>
            if device.revoked_at.is_some() {
                <span class="tagpill status-withdrawn">"Removed"</span>
            } else {
                <form method="post" action=(revoke)>
                    <button class="btn btn-ghost device-remove" type="submit">"Remove"</button>
                </form>
            }
        </li>
    })
}

/// One claim on My TINs: its kind and status, where it is in the review,
/// and what the person can do next.
#[component]
async fn tin_card(claim: ClaimRow) -> Result<impl View> {
    let (label, status_icon, tone) = status_look(&claim.status);
    let status = claim.status.clone();
    let checked = match status.as_str() {
        "needs_fix" => ("Needs a fix", "warn"),
        "rejected" => ("Not approved", "warn"),
        "approved" => ("Checked", "done"),
        _ => ("Checked", ""),
    };
    let papers = if claim.kind == "personal" { "your TIN ID" } else { "your papers" };
    let reasons: Vec<&str> =
        claim.fix_reasons.iter().filter_map(|code| claims::fix_reason(code)).map(|(_, message)| message).collect();
    let withdraw = format!("/tins/claims/{}/withdraw", claim.id);
    let fix = format!("/tins/claims/{}/fix", claim.id);
    let files = format!("/tins/claims/{}/files", claim.id);
    let delete = format!("/tins/claims/{}/delete", claim.id);
    let remove = format!("/tins/claims/{}/remove", claim.id);
    let tin = claim.tin().to_string();
    Ok(view! {
        <article class="tin-card">
            <div class="tin-card-top">
                kind_pill(kind: claim.kind.clone())
                pill(label: label.to_owned(), icon_name: status_icon, tone: format!("status-{tone}"))
            </div>
            <div class="tin-who">
                tin_text(tin: claim.tin().to_string())
                <span class="tin-name">(claim.registered_name.clone())</span>
                if let Some(trade) = claim.trade_name.clone() {
                    <span class="tin-trade">(trade)</span>
                }
            </div>
            <ol class="track" aria-label="Review progress">
                <li class="done">"Sent"</li>
                <li class=(checked.1)>(checked.0)</li>
                <li class=(match status.as_str() { "approved" => "done", "revoked" => "warn", _ => "" })>"Active"</li>
            </ol>
            match status.as_str() {
                "approved" => <p class="tin-note active">"Active since " (claim.decided_at.map(date).unwrap_or_default()) ". File from the Buwiz app on your computer."</p>,
                "pending" => <p class="tin-note waiting">"A person is checking " (papers) ", usually within one business day. You can already prepare returns in the app."</p>,
                "needs_fix" => {
                    <div class="tin-note fix">
                        <p><strong>"From our reviewer:"</strong></p>
                        <ul>
                            for reason in reasons {
                                <li>(reason)</li>
                            }
                        </ul>
                        if let Some(note) = claim.fix_note.clone() {
                            <p>(note)</p>
                        }
                    </div>
                },
                "revoked" => {
                    <div class="tin-note fix">
                        <p><strong>"No longer active."</strong> " Our staff ended this ownership on " (claim.removed_at.map(date).unwrap_or_default()) "."</p>
                        if let Some(note) = claim.removal_note.clone() {
                            <p>(note)</p>
                        }
                        <p>"You can add the TIN again with new papers."</p>
                    </div>
                },
                _ => {
                    <div class="tin-note fix">
                        <p><strong>"Not approved."</strong> " " (claim.decision_note.clone().unwrap_or_default())</p>
                    </div>
                },
            }
            match status.as_str() {
                "pending" | "needs_fix" => {
                    <div class="tin-actions">
                        if status == "needs_fix" {
                            <a class="btn btn-solid btn-small" href=(fix)>icon(name: "upload_file") "Fix and send again"</a>
                        }
                        <a class="btn btn-ghost btn-small" href=(files)>icon(name: "folder_open") "Files"</a>
                        <form method="post" action=(withdraw) data-confirm="Withdraw this claim? Its documents are deleted.">
                            <button class="btn btn-quiet btn-small" type="submit">"Withdraw"</button>
                        </form>
                        confirm_form(
                            label: "Delete",
                            action: delete,
                            question: "Delete this claim?".to_owned(),
                            detail: "The claim and every file you sent with it are erased for good.".to_owned(),
                            confirm: "Delete"
                        )
                    </div>
                },
                "rejected" => {
                    <div class="tin-actions">
                        confirm_form(
                            label: "Delete",
                            action: delete,
                            question: "Delete this claim?".to_owned(),
                            detail: "The claim and any file still kept with it are erased for good.".to_owned(),
                            confirm: "Delete"
                        )
                    </div>
                },
                "approved" => {
                    <div class="tin-actions">
                        confirm_form(
                            label: "Remove this TIN",
                            action: remove,
                            question: format!("Remove {tin}?"),
                            detail: "It stops being active and the app can't send returns for it. Sent returns stay in your history; add it again any time.".to_owned(),
                            confirm: "Remove"
                        )
                    </div>
                },
                _ => "",
            }
        </article>
    })
}

/// "Step 2 of 3 · Details" and the three-part bar.
#[component]
async fn progress(step: u8, label: &'static str, rest: &'static str) -> Result<impl View> {
    Ok(view! {
        <div class="progress">
            <div class="progress-label">
                <span class="progress-now">"Step " (step.to_string()) " of 3 · " (label)</span>
                <span>(rest)</span>
            </div>
            <div class="progress-bar">
                <span class="on"></span>
                <span class=(if step >= 2 { "on" } else { "" })></span>
                <span class=(if step >= 3 { "on" } else { "" })></span>
            </div>
        </div>
    })
}

/// A file field drawn as a drop zone. With JavaScript the native input
/// covers the zone invisibly (so dropping a file works) and the chosen
/// file's name shows; `data-required` becomes `required` when the slot's
/// kind of TIN is picked.
#[component]
async fn file_slot(
    field: &'static str,
    title: &'static str,
    hint: &'static str,
    icon_name: &'static str,
    #[default] required: bool,
    #[default] selfie: bool,
    #[default] multiple: bool,
) -> Result<impl View> {
    let accept = if selfie { "image/*" } else { "application/pdf,image/*" };
    Ok(view! {
        <div class=(if required { "slot required" } else { "slot" })>
            <label class="slot-drop" for=(field)>
                <span class="slot-icon">icon(name: icon_name)</span>
                <span class="slot-text">
                    <span class="slot-title">
                        (title)
                        if required {
                            <span class="slot-tag">"Required"</span>
                        } else {
                            <span class="slot-tag optional">"Optional"</span>
                        }
                    </span>
                    <span class="slot-hint">(hint)</span>
                    <span class="slot-file" data-file=""></span>
                </span>
                <span class="slot-action" data-action="">"Choose a file"</span>
                <input
                    id=(field)
                    type="file"
                    name=(field)
                    accept=(accept)
                    multiple=(multiple)
                    if selfie {
                        capture="user"
                    }
                    if required {
                        data-required="true"
                    }
                >
            </label>
        </div>
    })
}

#[page("/tins/new")]
pub async fn new_tin_page(cx: &Cx) -> Result<impl View> {
    signed_in(cx).await?;
    Ok(view! {
        <main class="home page">
            <div class="page-in narrow">
                <a class="back" href="/tins">icon(name: "arrow_back") "My TINs"</a>
                page_notice(notice: notice(cx))
                <form class="addtin" method="post" action="/tins/claims" enctype="multipart/form-data" data-addtin="">
                    <section class="addtin-step" data-step="1">
                        progress(step: 1, label: "Type", rest: "Details · Documents")
                        <div class="step-head">
                            <h1>"Whose TIN is this?"</h1>
                            <p class="page-lead">"We'll only ask for the papers this kind of TIN has."</p>
                        </div>
                        <fieldset class="kinds">
                            <legend class="visually-hidden">"Kind of TIN"</legend>
                            kind_option(
                                value: "personal",
                                title: "Personal",
                                description: "An employee or individual taxpayer. Most employees never get a Form 2303, and that's fine.",
                                icon_name: "person",
                                needs: vec!["TIN ID (digital or card)"],
                                checked: true
                            )
                            kind_option(
                                value: "business",
                                title: "Business",
                                description: "Any TIN with a Form 2303: freelancer, professional, sole proprietor, corporation or partnership.",
                                icon_name: "storefront",
                                needs: vec!["Form 2303", "Government ID"],
                                checked: false
                            )
                        </fieldset>
                        <p class="hint-box for-personal">icon(name: "info") <span>"Get the free digital TIN ID from " <a href=(ORUS) target="_blank" rel="noopener">"ORUS"</a> " if you don't have the card. It counts as a government ID."</span></p>
                        <p class="hint-box for-business">icon(name: "info") <span>"If the Form 2303 is in a company's name instead of yours, we'll also ask for proof you can act for it."</span></p>
                        <div class="step-actions">
                            <a class="btn btn-ghost" href="/tins">"Cancel"</a>
                            <button class="btn btn-solid" type="button" data-next="" hidden=(true)>"Continue" icon(name: "arrow_forward")</button>
                        </div>
                    </section>

                    <section class="addtin-step" data-step="2">
                        progress(step: 2, label: "Details", rest: "Documents")
                        <div class="details-grid">
                            <div class="panel details-card">
                                <div class="step-head">
                                    <span class="for-personal">kind_pill(kind: "personal".to_owned())</span>
                                    <span class="for-business">kind_pill(kind: "business".to_owned())</span>
                                    <h2 class="step-title">"Your TIN details"</h2>
                                    <p class="page-lead">
                                        "Type them exactly as they appear on your "
                                        <span class="for-personal">"TIN ID."</span>
                                        <span class="for-business">"Form 2303."</span>
                                    </p>
                                </div>
                                <div class="field-row">
                                    <div class="field field-tin">
                                        <label for="tin">"TIN"</label>
                                        <input id="tin" class="mono" type="text" name="tin" inputmode="numeric" autocomplete="off" placeholder="000-000-000" maxlength="20" required=(true) aria-describedby="tin-help" data-tin="">
                                        <span id="tin-help" class="field-help" data-tin-help="">"Nine digits. Dashes are added for you."</span>
                                    </div>
                                    <div class="field field-branch">
                                        <label for="branch">"Branch code " <span class="tag">"DEFAULT"</span></label>
                                        <input id="branch" class="mono" type="text" name="branch" inputmode="numeric" autocomplete="off" value="00000" maxlength="5" aria-describedby="branch-help">
                                        <span id="branch-help" class="field-help">"00000 is the head office. Change it only for a branch."</span>
                                    </div>
                                </div>
                                <div class="field">
                                    <label for="registered_name">"Registered name"</label>
                                    <input id="registered_name" type="text" name="registered_name" maxlength="200" autocomplete="name" required=(true) data-echo-source="name">
                                    <span class="field-help for-personal">"Your name as printed on your TIN ID."</span>
                                    <span class="field-help for-business">"The taxpayer name on the 2303, usually last name first."</span>
                                </div>
                                <div class="field for-business">
                                    <label for="trade_name">"Trade name " <span class="optional">"(optional)"</span></label>
                                    <input id="trade_name" type="text" name="trade_name" maxlength="200" autocomplete="organization">
                                </div>
                                <div class="step-actions">
                                    <button class="btn btn-ghost" type="button" data-back="" hidden=(true)>"Back"</button>
                                    <button class="btn btn-solid" type="button" data-next="" hidden=(true)>"Continue" icon(name: "arrow_forward")</button>
                                </div>
                            </div>
                            <aside class="panel where">
                                <h2>"Where to find these"</h2>
                                <div class="paper" aria-hidden="true">
                                    <span class="paper-title for-personal">"TIN ID"</span>
                                    <span class="paper-title for-business">"Form 2303 · Certificate of Registration"</span>
                                    <span class="paper-line marked"><span>"TIN"</span><b class="mono" data-echo="tin">"—"</b></span>
                                    <span class="paper-line marked"><span>"Taxpayer name"</span><b data-echo="name">"—"</b></span>
                                    <span class="paper-line for-business"><span>"Trade name"</span><b>"—"</b></span>
                                    <span class="paper-bar"></span>
                                    <span class="paper-bar short"></span>
                                </div>
                                <p class="where-note">"The highlighted lines are the ones we compare. A different spelling is the most common reason a claim is sent back."</p>
                            </aside>
                        </div>
                    </section>

                    <section class="addtin-step" data-step="3">
                        progress(step: 3, label: "Documents", rest: "Almost done")
                        <div class="for-personal">
                            <div class="step-head">
                                kind_pill(kind: "personal".to_owned())
                                <h2 class="step-title">"Upload your TIN ID"</h2>
                                <p class="page-lead">"One photo or file. The card or the digital TIN ID both work. A selfie or your National ID makes the check stronger."</p>
                            </div>
                            <div class="slots-split">
                                file_slot(field: "tin_id", title: "Your TIN ID", hint: "The card, or the digital TIN ID file from ORUS. PDF or photo, up to 10 MB.", icon_name: "badge", required: true)
                                <aside class="panel tips">
                                    <p class="tips-title">"A good photo"</p>
                                    <p>icon(name: "crop_free") "All four corners in the frame"</p>
                                    <p>icon(name: "light_mode") "No glare over the name or TIN"</p>
                                    <p>icon(name: "picture_as_pdf") "The digital TIN ID file from ORUS works as is"</p>
                                    <p class="tips-orus">"No TIN ID yet? " <a href=(ORUS) target="_blank" rel="noopener">"Get the free digital one from ORUS"</a></p>
                                </aside>
                            </div>
                            <p class="slots-label">"Make the check stronger"</p>
                            <div class="slots-pair">
                                file_slot(field: "selfie", title: "Add a selfie", hint: "Face the camera in good light. We compare it with the photo on your TIN ID.", icon_name: "face", selfie: true)
                                file_slot(field: "national_id", title: "Add your National ID", hint: "A photo of your PhilSys card or ePhilID, with its QR code readable.", icon_name: "id_card")
                            </div>
                        </div>
                        <div class="for-business">
                            <div class="step-head">
                                kind_pill(kind: "business".to_owned())
                                <h2 class="step-title">"Your business papers"</h2>
                                <p class="page-lead">"A person checks everything, usually within one business day."</p>
                            </div>
                            <div class="slots">
                                file_slot(field: "form_2303", title: "Form 2303", hint: "Your Certificate of Registration. PDF or photo, up to 10 MB.", icon_name: "description", required: true)
                                file_slot(field: "government_id", title: "Your government ID", hint: "Passport, driver's license, UMID, PhilSys or another ID with your photo.", icon_name: "badge", required: true)
                                <div class="authority">
                                    file_slot(field: "authority", title: "Proof you can act for it", hint: "Needed when the Form 2303 is in a company's name rather than yours. Any one of these, naming you:", icon_name: "gavel")
                                    <p class="chips" aria-label="Accepted proof">
                                        <span>"Secretary's certificate"</span>
                                        <span>"Board resolution"</span>
                                        <span>"Special power of attorney"</span>
                                    </p>
                                </div>
                            </div>
                        </div>
                        <details class="panel more-papers">
                            <summary>"Add other papers " <span class="optional">"(optional: SEC registration, DTI papers, up to 5)"</span></summary>
                            file_slot(field: "supporting", title: "Other papers", hint: "PDF or photos, up to 10 MB each.", icon_name: "attach_file", multiple: true)
                        </details>
                        <div class="send-bar">
                            <span class="send-what">
                                <span class="mono" data-echo="tin">"Your TIN"</span>
                                <span class="send-sub" data-summary="">"Check your files, then send them."</span>
                            </span>
                            <span class="send-buttons">
                                <button class="btn btn-ghost-dark" type="button" data-back="" hidden=(true)>"Back"</button>
                                <button class="btn btn-send" type="submit">"Send for review" icon(name: "send")</button>
                            </span>
                        </div>
                        <p class="fine">icon(name: "bolt") "You can start preparing returns in the app right away. Sending to the BIR waits for the check."</p>
                    </section>
                    <p class="fine">icon(name: "lock") "Files are encrypted and deleted after review."</p>
                </form>
            </div>
        </main>
    })
}

/// One answer to "Whose TIN is this?", drawn as a card around its radio.
#[component]
async fn kind_option(
    value: &'static str,
    title: &'static str,
    description: &'static str,
    icon_name: &'static str,
    needs: Vec<&'static str>,
    checked: bool,
) -> Result<impl View> {
    let id = format!("kind-{value}");
    Ok(view! {
        <label class="kind" for=(id.clone())>
            <input id=(id) type="radio" name="kind" value=(value) checked=(checked) required=(true)>
            <span class="kind-icon">icon(name: icon_name)</span>
            <span class="kind-text">
                <span class="kind-title">(title)</span>
                <span class="kind-desc">(description)</span>
                <span class="kind-needs">
                    for need in needs {
                        <span>icon(name: "description") (need)</span>
                    }
                </span>
            </span>
            <span class="kind-radio" aria-hidden="true"></span>
        </label>
    })
}

/// One stored file on the claim's files page: replace it, or remove it
/// when the claim can do without it.
#[component]
async fn file_row(claim_id: Uuid, document: DocumentRow, required: bool) -> Result<impl View> {
    let base = format!("/tins/claims/{claim_id}/documents/{}", document.id);
    let accept = if document.kind == "selfie" { "image/*" } else { "application/pdf,image/*" };
    let size = format!("{} KB", (document.size_bytes + 1023) / 1024);
    Ok(view! {
        <li class="file-row">
            <span class="file-icon">icon(name: DocumentKind::icon(&document.kind))</span>
            <span class="file-text">
                <b>
                    (DocumentKind::label(&document.kind))
                    if required {
                        <span class="slot-tag">"Required"</span>
                    }
                </b>
                <span class="file-name">(document.filename.clone())</span>
                <span>(size) " · added " (date_time(document.created_at))</span>
            </span>
            <div class="file-actions">
                <form
                    class="file-replace"
                    method="post"
                    action=(format!("{base}/replace"))
                    enctype="multipart/form-data"
                    data-confirm="Replace this file?"
                    data-confirm-detail="The new file takes its place, and this one is deleted for good."
                    data-confirm-label="Replace"
                    data-confirm-tone="danger"
                >
                    <input type="file" name=(document.kind.clone()) accept=(accept) required=(true) aria-label=(format!("New {}", DocumentKind::label(&document.kind))) >
                    <button class="btn btn-ghost btn-small" type="submit">icon(name: "upload_file") "Replace"</button>
                </form>
                if !required {
                    confirm_form(
                        label: "Remove",
                        action: format!("{base}/remove"),
                        question: "Remove this file?".to_owned(),
                        detail: "It is deleted for good. The claim keeps its other files.".to_owned(),
                        confirm: "Remove"
                    )
                }
            </div>
        </li>
    })
}

pub mod claim_id {
    use super::*;

    /// The files of a claim still open, one at a time: replace any of
    /// them, remove the optional ones.
    #[page("/tins/claims/{claim_id}/files")]
    pub async fn files_page(cx: &Cx) -> Result<impl View> {
        let account = signed_in(cx).await?;
        let claim_id = path_id(cx, 1)?;
        let db = &state(cx).db;
        let claim = match claims::find(db, claim_id).await? {
            Some(claim) if claim.account_id == account.id => claim,
            _ => return Err(topcoat::router::error::not_found().into()),
        };
        if !claim.is_open() {
            return Err(redirect(link("/tins", Some(Notice::FilesLocked), &[])).into());
        }
        let kind = ClaimKind::parse(&claim.kind).unwrap_or(ClaimKind::Business);
        let rows: Vec<(DocumentRow, bool)> = claims::documents(db, claim_id)
            .await?
            .into_iter()
            .filter(|document| document.purged_at.is_none())
            .map(|document| {
                let required = kind.required().iter().any(|required| required.as_str() == document.kind);
                (document, required)
            })
            .collect();
        let fix = format!("/tins/claims/{claim_id}/fix");
        Ok(view! {
            <main class="home page">
                <div class="page-in narrow">
                    <a class="back" href="/tins">icon(name: "arrow_back") "My TINs"</a>
                    page_notice(notice: notice(cx))
                    <div class="step-head">
                        kind_pill(kind: claim.kind.clone())
                        <h1>"Your files"</h1>
                        <p class="page-lead">tin_text(tin: claim.tin().to_string()) " · " (claim.registered_name.clone())</p>
                    </div>
                    if claim.status == "needs_fix" {
                        <p class="hint-box">icon(name: "info") <span>"When your files are right, " <a href=(fix)>"send the claim again"</a> " so the reviewer checks it."</span></p>
                    } else {
                        <p class="hint-box">icon(name: "info") <span>"The reviewer sees your changes, and that you made them after sending the claim."</span></p>
                    }
                    if rows.is_empty() {
                        <p class="panel empty">"No files on this claim."</p>
                    } else {
                        <ul class="panel file-rows">
                            for (document, required) in rows {
                                file_row(claim_id: claim_id, document: document, required: required)
                            }
                        </ul>
                    }
                    <p class="fine">icon(name: "lock") "Files are encrypted. A replaced or removed file is deleted at once."</p>
                </div>
            </main>
        })
    }

    /// A claim staff sent back: what they asked for, the files on it, and
    /// a form to replace files or correct the name and send it again.
    #[page("/tins/claims/{claim_id}/fix")]
    pub async fn fix_page(cx: &Cx) -> Result<impl View> {
        let account = signed_in(cx).await?;
        let claim_id = path_id(cx, 1)?;
        let db = &state(cx).db;
        let claim = match claims::find(db, claim_id).await? {
            Some(claim) if claim.account_id == account.id => claim,
            _ => return Err(topcoat::router::error::not_found().into()),
        };
        if claim.status != "needs_fix" {
            return Err(redirect(link("/tins", Some(Notice::NotSentBack), &[])).into());
        }
        let documents: Vec<DocumentRow> = claims::documents(db, claim_id)
            .await?
            .into_iter()
            .filter(|document| document.purged_at.is_none())
            .collect();
        let reasons: Vec<&str> =
            claim.fix_reasons.iter().filter_map(|code| claims::fix_reason(code)).map(|(_, message)| message).collect();
        let personal = claim.kind == "personal";
        let action = format!("/tins/claims/{claim_id}/resend");
        Ok(view! {
            <main class="home page">
                <div class="page-in narrow">
                    <a class="back" href="/tins">icon(name: "arrow_back") "My TINs"</a>
                    page_notice(notice: notice(cx))
                    <div class="step-head">
                        kind_pill(kind: claim.kind.clone())
                        <h1>"Fix and send again"</h1>
                        <p class="page-lead">tin_text(tin: claim.tin().to_string()) " · " (claim.registered_name.clone())</p>
                    </div>
                    <div class="tin-note fix">
                        <p><strong>"From our reviewer:"</strong></p>
                        <ul>
                            for reason in reasons {
                                <li>(reason)</li>
                            }
                        </ul>
                        if let Some(note) = claim.fix_note.clone() {
                            <p>(note)</p>
                        }
                    </div>
                    <form class="addtin fix-form" method="post" action=(action) enctype="multipart/form-data">
                        <div class="panel details-card">
                            <h2 class="step-title">"Names"</h2>
                            <div class="field">
                                <label for="registered_name">"Registered name"</label>
                                <input id="registered_name" type="text" name="registered_name" maxlength="200" value=(claim.registered_name.clone()) required=(true)>
                            </div>
                            if !personal {
                                <div class="field">
                                    <label for="trade_name">"Trade name " <span class="optional">"(optional)"</span></label>
                                    <input id="trade_name" type="text" name="trade_name" maxlength="200" value=(claim.trade_name.clone().unwrap_or_default())>
                                </div>
                            }
                        </div>
                        <div class="panel details-card">
                            <h2 class="step-title">"Files"</h2>
                            <ul class="file-list">
                                for document in documents {
                                    <li>icon(name: DocumentKind::icon(&document.kind)) <b>(DocumentKind::label(&document.kind))</b> <span>(document.filename.clone())</span></li>
                                }
                            </ul>
                            <p class="field-help">"A new file replaces the one of the same kind. Other papers are added."</p>
                            <div class="slots">
                                if personal {
                                    file_slot(field: "tin_id", title: "New TIN ID", hint: "The card, or the digital TIN ID file from ORUS.", icon_name: "badge")
                                    file_slot(field: "selfie", title: "New selfie", hint: "Face the camera in good light.", icon_name: "face", selfie: true)
                                    file_slot(field: "national_id", title: "New National ID photo", hint: "With its QR code readable.", icon_name: "id_card")
                                } else {
                                    file_slot(field: "form_2303", title: "New Form 2303", hint: "PDF or photo, up to 10 MB.", icon_name: "description")
                                    file_slot(field: "government_id", title: "New government ID", hint: "An ID with your photo.", icon_name: "badge")
                                    file_slot(field: "authority", title: "Proof you can act for it", hint: "Secretary's certificate, board resolution or SPA naming you.", icon_name: "gavel")
                                }
                                file_slot(field: "supporting", title: "Other papers", hint: "Added to the claim.", icon_name: "attach_file", multiple: true)
                            </div>
                        </div>
                        <div class="send-bar">
                            <span class="send-what">
                                tin_text(tin: claim.tin().to_string())
                                <span class="send-sub">"It goes back to the same reviewer queue."</span>
                            </span>
                            <span class="send-buttons">
                                <button class="btn btn-send" type="submit">"Send again" icon(name: "send")</button>
                            </span>
                        </div>
                        <p class="fine">icon(name: "lock") "Files are encrypted and deleted after review."</p>
                    </form>
                </div>
            </main>
        })
    }
}
