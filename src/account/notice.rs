//! Messages shown to people after a form post. Redirects carry only the
//! code (`?notice=tin_digits`), so a crafted link cannot put its own words
//! on our pages.

macro_rules! notices {
    ($($variant:ident = $code:literal => $tone:ident $message:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Notice {
            $($variant,)*
        }

        impl Notice {
            pub fn code(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)*
                }
            }

            pub fn message(self) -> &'static str {
                match self {
                    $(Self::$variant => $message,)*
                }
            }

            pub fn is_error(self) -> bool {
                match self {
                    $(Self::$variant => notices!(@error $tone),)*
                }
            }

            pub fn from_code(code: &str) -> Option<Self> {
                match code {
                    $($code => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
    (@error error) => { true };
    (@error info) => { false };
}

notices! {
    EmailInvalid = "email_invalid" => error "Enter a valid email address.",
    CodeSent = "code_sent" => info "Please check your email",
    CodeTooMany = "code_too_many" => error "Too many codes were sent to this address. Try again in an hour.",
    CodeInvalid = "code_invalid" => error "That code is wrong or has expired. Request a new one.",
    SignedOut = "signed_out" => info "You are signed out.",
    SignInFirst = "sign_in_first" => info "Sign in to continue.",
    DeviceInvalid = "device_invalid" => error "That code is wrong or has expired. Start the sign-in again in the app.",
    DeviceApproved = "device_approved" => info "Done. Go back to the app; it is signing you in.",
    DeviceDenied = "device_denied" => info "Sign-in declined. The app was not signed in.",
    DeviceRevoked = "device_revoked" => info "Device removed. It can no longer send returns; sign in again on it to use it.",
    TinDigits = "tin_digits" => error "The TIN must have 9 digits.",
    TinZero = "tin_zero" => error "That TIN is not valid.",
    TinBranch = "tin_branch" => error "The branch code must have 3 or 5 digits (head office is 00000).",
    KindMissing = "kind_missing" => error "Choose whether this is a personal or a business TIN.",
    NameMissing = "name_missing" => error "Enter the registered name exactly as it appears on your document.",
    Need2303 = "need_2303" => error "Attach a scan or photo of your BIR Form 2303.",
    NeedId = "need_id" => error "Attach a scan or photo of a government ID.",
    NeedTinId = "need_tin_id" => error "Attach a photo of your TIN ID, or the digital TIN ID file from ORUS.",
    WrongDocuments = "wrong_documents" => error "Attach only the papers asked for this kind of TIN.",
    TooManySupporting = "too_many_supporting" => error "Attach at most 5 supporting documents.",
    FileTooBig = "file_too_big" => error "Each file must be 10 MB or smaller, and all files together 40 MB or smaller.",
    FileType = "file_type" => error "Files must be PDF, JPEG, PNG, WebP or HEIC.",
    BadUpload = "bad_upload" => error "The upload did not finish. Try again.",
    TinAlreadyActive = "tin_already_active" => error "This TIN is already active on your account.",
    ClaimAlreadyPending = "claim_already_pending" => error "You already have a claim for this TIN waiting for review.",
    ClaimSubmitted = "claim_submitted" => info "Thanks. We will review your documents and email you when the TIN is active.",
    ClaimWithdrawn = "claim_withdrawn" => info "Claim withdrawn. Its documents were deleted.",
    ClaimResent = "claim_resent" => info "Thanks. We will check it again, usually within one business day.",
    NotSentBack = "not_sent_back" => error "This claim is not waiting for a fix.",
    FixNeedsChange = "fix_needs_change" => error "Upload a new file or correct the name before sending it again.",
    ClaimMissing = "claim_missing" => error "That claim does not exist.",
    OwnClaim = "own_claim" => error "You cannot decide your own claim.",
    AlreadyDecided = "already_decided" => error "This claim was already decided.",
    RejectNeedsNote = "reject_needs_note" => error "Say why the claim is rejected; the claimant sees this note.",
    TinOwnedElsewhere = "tin_owned_elsewhere" => error "Another account already owns this TIN. Reject this claim, or end the other ownership first.",
    ClaimDecided = "claim_decided" => info "Decision saved. The claim's documents were deleted.",
    FixNeedsReason = "fix_needs_reason" => error "Pick a reason or write a note; the person sees it on My TINs.",
    ClaimSentBack = "claim_sent_back" => info "Sent back. The person sees your reasons on My TINs.",
    ClaimDeleted = "claim_deleted" => info "Claim deleted, with its files.",
    ClaimNotDeletable = "claim_not_deletable" => error "This TIN was active, so its record stays with the returns sent for it. Remove the TIN instead.",
    TinNotActive = "tin_not_active" => error "Only an active TIN can be removed or revoked.",
    TinRemoved = "tin_removed" => info "TIN removed. It is no longer active, and returns already sent stay in your history. You can add it again any time.",
    OwnershipRevoked = "ownership_revoked" => info "Ownership revoked. The TIN is no longer active on that account and can be claimed again.",
    ReasonMissing = "reason_missing" => error "Write a reason. It is kept in the account's history.",
    FilesLocked = "files_locked" => error "Files can change only while a claim is being checked or waiting for a fix.",
    DocumentMissing = "document_missing" => error "That file is no longer on the claim.",
    FileRequired = "file_required" => error "This document is required. Replace it with a new file instead.",
    ChooseFile = "choose_file" => error "Choose a file to upload.",
    FileReplaced = "file_replaced" => info "File replaced. The old one was deleted.",
    FileRemoved = "file_removed" => info "File removed and deleted.",
    SignedOutEverywhere = "signed_out_everywhere" => info "Signed out of every browser, app and AI agent.",
    AppSignedOut = "app_signed_out" => info "That app was signed out.",
    TinSaved = "tin_saved" => info "Saved as Not verified. You can prepare returns for this TIN now; verify it with its papers to send them.",
    TinAlreadyAdded = "tin_already_added" => error "This TIN is already on My TINs. Verify it there to send returns.",
    NotUnverified = "not_unverified" => error "This TIN already has its papers. See where it stands on My TINs.",
    TooManyUnverified = "too_many_unverified" => error "You have 20 TINs waiting to be verified. Verify or remove some first.",
    AdminOnly = "admin_only" => error "Only an admin can do this.",
    OwnAccount = "own_account" => error "You can't do this to your own account.",
    RoleChanged = "role_changed" => info "Role changed.",
    RoleUnknown = "role_unknown" => error "Pick Admin, Support or No staff role.",
    LastAdmin = "last_admin" => error "Buwiz needs at least one admin. Make someone else an admin first.",
    RoleFromServer = "role_from_server" => error "This person is an admin through the server setting ADMIN_EMAILS. Change it there.",
    AccountMissing = "account_missing" => error "That account does not exist.",
    AccountBanned = "account_banned" => error "This account can't be used on Buwiz. If you think this is a mistake, write to support@buwiz.com.",
    AccountBannedStaff = "account_banned_staff" => info "Account banned. It can't sign in or send until you lift the ban.",
    BanLifted = "ban_lifted" => info "Ban lifted. The person can sign in again.",
    BanAdmin = "ban_admin" => error "Change this person's role first; an admin can't be banned.",
    TinActivated = "tin_activated" => info "Activated without papers. The reason is in the account's history.",
    ClaimantBanned = "claimant_banned" => error "This account is banned. Lift the ban before making its TIN active.",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for notice in [Notice::EmailInvalid, Notice::TinBranch, Notice::ClaimDecided] {
            assert_eq!(Notice::from_code(notice.code()), Some(notice));
        }
        assert_eq!(Notice::from_code("<script>"), None);
    }
}
