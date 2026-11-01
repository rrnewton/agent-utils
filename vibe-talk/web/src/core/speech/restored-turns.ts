// Whether a recovered spoken turn should remain saved for the next opening to retry.
// #624 voice-page-modularity.

/**
 * Is a failure to file a recovered turn one that the next opening could get past? A connection that
 * failed, a server that is down, a token it refused: yes, and the copy goes back. A server with no
 * store, or one that refuses the turn itself, would refuse it at every opening: the copy is let go.
 */
function recoveryMayRetry(error: {status?: number; code?: string} | null) {
  const status = error && typeof error.status === "number" ? error.status : null;
  if (status === null) return true;
  if (error!.code === "storage_not_configured") return false;
  return status >= 500 || status === 401 || status === 403 || status === 408 || status === 429;
}

export { recoveryMayRetry };
