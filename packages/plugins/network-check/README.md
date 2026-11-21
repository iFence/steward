# Network Check

This official validation plugin exercises Steward's scoped HTTPS capability.
It requests only `GET https://api.github.com/rate_limit`; network declarations
remain requests until the user grants both the named host call and the exact
HTTP scope.
