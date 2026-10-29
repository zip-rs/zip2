# Security Policy

## Supported Versions

Only the latest released version (and, when a prerelease is newer than the latest stable release, that prerelease to a 
limited extent) are supported. Backports of security fixes or backward-compatibility shims for older major versions may
be developed, but this is strictly best-effort; all users should update to the latest version as soon as possible.

## Reporting a Vulnerability

To report a vulnerability in a prerelease version or at the tip of the master branch, if no release versions are affected,
please open an issue at https://github.com/zip-rs/zip2/issues. Prerelease versions and tip-of-master builds aren't expected
to be secure, so full disclosure of vulnerabilities that are caught at that stage doesn't cost us trust, and it makes it
easier to develop a fix collaboratively.

To report a vulnerability in a release that's published on crates.io, please go to 
https://github.com/zip-rs/zip2/security/advisories/new. We'll attempt to:

* Close the report within 7 days without publishing it if:
  * It's invalid.
  * It's a duplicate of another report we've already accepted as a triaged draft (we want each draft to correspond to a
    separate issue).
  * It's an issue in a dependency (in which case it should be reported to the dependency's maintainers; we don't want to
    duplicate the report they'll publish), unless that dependency is unmaintained.
* Accept the report as a draft within 7 days if none of those are the case.
* Provide progress reports at least every 7 days to the original reporter.
* Aim to provide a fix within 30 days of the initial report. If a complete fix is not feasible in that timeframe (for
  example, due to complexity or external dependencies), we will communicate this to the reporter, share any available 
  mitigations or workarounds, and adjust the expected timeline accordingly.

## Disclosure

A vulnerability that affects a published version will only be publicly disclosed once a version without the vulnerability has 
been published, which is not a prerelease unless all affected published versions were also prereleases. Once that's done, 
the delay before full public disclosure will be determined as follows:

* If the proof-of-concept is very simple, or an exploit is already in the wild (whether or not it specifically targets `zip`),
  all details will be made public right away.
* If the vulnerability is specific to `zip` and cannot easily be reverse-engineered from the code history, then the
  advisory will be withheld for another 14 days, although broad information such as "an algorithmic-complexity attack
  that will be published on 20XX-YY-ZZ was fixed in version x.y.z, and it affects certain users who only read ZIP files
  from untrusted sources into memory if they use an uncommon configuration of feature flags" may be shared.
* If a potential victim at credible risk requests more time to deploy a fix, then the withholding of details can
  be extended up to 30 days. This may be extended to 90 days for high-value government and nonprofit targets, when
  truly extraordinary circumstances are delaying the deployment.

## No yanked versions

Previous versions of this policy called for yanking affected versions from crates.io. However, this proved to be too
disruptive in the case of https://github.com/zip-rs/zip2/security/advisories/GHSA-vv5m-jxxh-4627 and
https://github.com/zip-rs/zip2/security/advisories/GHSA-4v9h-p7m9-rh26 because those vulnerabilities affected a wide range
of versions, and is not recommended by https://doc.rust-lang.org/cargo/commands/cargo-yank.html. Thus, versions will no 
longer be yanked based on security vulnerabilities.
