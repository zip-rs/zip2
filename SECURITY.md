# Security Policy

## Supported Versions

Only the latest released version is supported.

## Reporting a Vulnerability

To report a vulnerability, please go to https://github.com/zip-rs/zip2/security/advisories/new. We'll attempt to:

* Close the report within 7 days if it's invalid, or if a fix has already been released but some old versions needed to be yanked.
* Provide progress reports at least every 7 days to the original reporter.
* Aim to provide a fix within 30 days of the initial report. If a complete fix is not feasible in that timeframe (for example, due to complexity or external dependencies), we will communicate this to the reporter, share any available mitigations or workarounds, and adjust the expected timeline accordingly.

## Disclosure

A vulnerability that affects a published version will only be publicly disclosed once a version without the vulnerability has 
been published, which is not a prerelease unless all affected versions were prereleases, and the affected versions have been 
yanked. Once that's done, the delay before full public disclosure will be determined as follows:

* If the proof-of-concept is very simple, or an exploit is already in the wild (whether or not it specifically targets `zip`),
  all details will be made public right away.
* If the vulnerability is specific to `zip` and cannot easily be reverse-engineered from the code history, then the
  proof-of-concept and most of the details will be withheld for another 14 days.
* If a potential victim at credible risk requests more time to deploy a fix, then the withholding of details can
  be extended up to 30 days. This may be extended to 90 days for high-value government and nonprofit targets, when
  truly extraordinary circumstances are delaying the deployment.
