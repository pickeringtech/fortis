# Packages the documentation examples import (DEL-43, PROG-TST-08)

Some documentation examples import an example package (`import acme.accounts { User }`). They are not standalone
programs, so `conformance/tools/docs_check.py` runs each as the root module of a package that depends on these.
Each package declares only what the examples name. A source file whose first line is `// base: <ID>` is that
corpus fixture's declarations: `acme.billing`'s `invoices` module is the worked module of Language Reference
section 2 (`LR-2-01`), so the examples that import it see exactly the module the document shows.
