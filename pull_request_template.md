<!-- 
We welcome your pull request, but because this crate is downloaded about 1.7 million times per month (see https://crates.io/crates/zip),
and because ZIP file processing has caused security issues in the past (see 
https://www.cvedetails.com/vulnerability-search.php?f=1&vendor=&product=zip&cweid=&cvssscoremin=&cvssscoremax=&publishdatestart=&publishdateend=&updatedatestart=&updatedateend=&cisaaddstart=&cisaaddend=&cisaduestart=&cisadueend=&page=1
for the gory details), we have some requirements that help ensure the crate remains secure and panic-free, and that a lot of PRs 
don't meet.

We don't filter out "ZIP bombs" because extreme compression ratios and shallow file copies have legitimate uses; but
we expect the tools we provide for checking that extraction is safe, such as the `ZipArchive::decompressed_size` method in
https://github.com/zip-rs/zip2/blob/master/src/read.rs, to remain reliably effective. We also expect all the crate's methods to
remain panic-free, so that this crate can be used on servers without creating a denial-of-service vulnerability.

These are our requirements for PRs, in addition to the usual functionality and readability requirements:
- This codebase sometimes changes rapidly. Please rebase your branch before opening a pull request, and 
  grant @Pr0methean write access to the source branch (so he can fix later conflicts without being subject 
  to the limitations of the web UI) if EITHER of the following apply:
  - It has been at least 24 hours since you forked the repo or previously rebased the branch; or
  - 5 or more pull requests are already open at https://github.com/zip-rs/zip2/pulls. PRs are merged in the order they become
    eligible (reviewed, passing CI tests, and no conflicts with the base branch). @Pr0methean will attempt to fix merge
    conflicts, but this is best-effort.
- Please make sure the repo your PR targets is `zip-rs/zip2` and not `zip-rs/zip-old`. The latter
  repo is no longer maintained and will be archived once the pre-existing issues are closed.
- Your changes must build against the MSRV (see README.md) AND the latest stable Rust version AND the latest nightly Rust version, 
  with `--no-default-features` AND with `--all-features` AND with the default features.
- PRs must pass `cargo clippy --all-targets` and `cargo fmt --check --all`,
  with `--no-default-features` AND with `--all-features` AND with the default features.
  If you need to add a new `#[allow]` attribute, please place a comment on the same line or just above it, explaining what the
  exception applies to and why it's needed.
- Commit messages and the PR title must conform to [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) and start 
  with one of the types specified by the [Angular convention](https://github.com/angular/angular/blob/22b96b9/CONTRIBUTING.md#type).
- All commits must be signed and display a "Verified" badge; see 
  https://docs.github.com/en/authentication/managing-commit-signature-verification/about-commit-signature-verification.
  If any of your commits don't have a "Verified" badge, here's how to fix this:
  1. Generate a GPG key if you don't already have one, by following
     https://docs.github.com/en/authentication/managing-commit-signature-verification/generating-a-new-gpg-key.
  2. If you use GitHub's email privacy feature, associate the key with your users.noreply.github.com email address by following
     https://docs.github.com/en/authentication/managing-commit-signature-verification/associating-an-email-with-your-gpg-key.
  3. Configure Git to use your signing key by following
     https://docs.github.com/en/authentication/managing-commit-signature-verification/telling-git-about-your-signing-key
  4. Add the key to your GitHub account by following
     https://docs.github.com/en/authentication/managing-commit-signature-verification/adding-a-gpg-key-to-your-github-account
  5. Enable commit signing by following
     https://docs.github.com/en/authentication/managing-commit-signature-verification/signing-commits
  6. Squash your PR into one commit or run `git commit --amend --no-edit`, because enabling commit signing isn't retroactive
     even for unpushed commits.
-->
