# NSH GPUI backend compatibility patch

This directory vendors `nsh-platform-gpui` 0.5.1 from crates.io.

The only source change removes the manual `IntoElement` implementations for
`SurfaceElement` and `WindowElement`. GPUI Fast 0.1.16 supplies those through
the blanket implementation for `Element`, so retaining both implementations
causes conflicting trait implementations.

Remove this patch and the `[patch.crates-io]` entry in the workspace manifest
after NSH publishes an equivalent compatibility fix.
