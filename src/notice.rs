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
    CodeSent = "code_sent" => info "We emailed you a 6-digit code. It works for 10 minutes.",
    CodeTooMany = "code_too_many" => error "Too many codes were sent to this address. Try again in an hour.",
    CodeInvalid = "code_invalid" => error "That code is wrong or has expired. Request a new one.",
    SignedOut = "signed_out" => info "You are signed out.",
    SignInFirst = "sign_in_first" => info "Sign in to continue.",
    DeviceInvalid = "device_invalid" => error "That code is wrong or has expired. Start the sign-in again in the app.",
    DeviceApproved = "device_approved" => info "Done. Go back to the app; it is signing you in.",
    DeviceDenied = "device_denied" => info "Sign-in declined. The app was not signed in.",
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
    SignedOutEverywhere = "signed_out_everywhere" => info "Signed out of every browser, app and AI agent.",
    AppSignedOut = "app_signed_out" => info "That app was signed out.",
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
