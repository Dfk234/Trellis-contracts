use soroban_sdk::contracterror;

/// Oracle-specific error codes (range 500-599 per shared conventions).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OracleError {
    /// The submitter is not registered or not active.
    SubmitterNotAuthorized = 500,
    /// The submission was already submitted (replay detected).
    DuplicateSubmission = 501,
    /// The feed does not exist or has no submissions.
    FeedNotFound = 502,
    /// The submitter is already registered.
    SubmitterAlreadyRegistered = 503,
    /// The submission is stale (outside the staleness window).
    SubmissionStale = 504,
    /// The price value is invalid (e.g., negative for certain feeds).
    InvalidPrice = 505,
    /// The decimal places value is invalid.
    InvalidDecimals = 506,
    /// The feed identifier is invalid (e.g., empty).
    InvalidFeedId = 507,
    /// The caller is not authorized to perform this action.
    Unauthorized = 508,
    /// A generic error occurred.
    InternalError = 509,
    /// The submission timestamp is too far in the future.
    SubmissionFromFuture = 510,
    /// The feed does not have enough active submitters for aggregation.
    QuorumNotMet = 511,
}
