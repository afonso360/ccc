# CCC resource directory

This directory contains versioned compiler-owned headers and, when required,
target runtime shims. The driver validates `manifest.toml` before adding the
resource include directory to the configured include search.

Only headers whose declarations or macro contracts can be described truthfully
by the effective compilation configuration belong here. Platform ABI headers
remain owned by the resolved target libc and use wrappers only when CCC must
supply compiler builtins.

The manifest also records the GNU compatibility profile used solely to select
hosted-header preprocessing paths. Its checked capability and declined-feature
sets keep the advertised compiler version tied to behavior the preprocessor
actually implements; later GNU syntax is not implied by this profile.
