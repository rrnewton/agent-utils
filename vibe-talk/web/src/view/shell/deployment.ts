// Which deployment's origin and host the page shows, and redacting the browser's token
// from anything it might otherwise print (`#624 voice-page-modularity`, moved from the
// legacy script). Words and order are as they were in the script. The token and the
// token field are the page's, handed in as they stand, so what is redacted says the same.

/** What the deployment words and redacting ask of the page. */
export interface DeploymentWordsDependencies {
  token(): string;
  tokenField(): { value: string };
}

/** The deployment's origin and host, and redacted text. */
export interface DeploymentWords {
  deploymentOrigin(): string;
  deploymentHost(): string;
  redact(text: unknown): string;
}

/** The deployment words over `deps`. */
export function createDeploymentWords(deps: DeploymentWordsDependencies): DeploymentWords {
  /** The origin receiving the token and API requests, shown before and after sign-in. */
  function deploymentOrigin(): string {
    const origin = window.location && typeof window.location.origin === "string"
      ? window.location.origin.trim()
      : "";
    return origin || "this server";
  }

  /** The host this page was opened at, port included: `#342 settings-server-hostname`. */
  function deploymentHost(): string {
    const host = window.location && typeof window.location.host === "string"
      ? window.location.host.trim()
      : "";
    if (host) return host;
    const origin = deploymentOrigin().replace(/^[a-z][a-z0-9+.-]*:\/\//i, "");
    return origin.split("/")[0] || deploymentOrigin();
  }

  // Nothing this page displays may contain a credential.
  //
  // The server redacts its own secrets before answering, but this page holds one the server has
  // never seen: the API token in this browser. An error message is assembled from whatever came
  // back, and a server — or something in between it and here — is free to echo a request back
  // verbatim. So the last thing before any text reaches the DOM is this: the token, if it is in
  // there, is replaced. It is one line, it costs nothing, and it removes a whole class of "the
  // error message leaked the key" from being possible at all.
  function redact(text: unknown): string {
    let out = String(text);
    const secrets = [deps.token(), deps.tokenField().value || ""];
    for (const secret of secrets) {
      // Short strings are not credentials, and blanket-replacing one would mangle ordinary words.
      if (secret.length >= 8) {
        out = out.split(secret).join("[redacted]");
      }
    }
    return out;
  }

  return { deploymentOrigin, deploymentHost, redact };
}
