<!-- 
We welcome your pull request, but we have some requirements that a lot of PRs don't meet:
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
