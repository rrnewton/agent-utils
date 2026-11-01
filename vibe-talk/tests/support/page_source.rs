// Source-text guards follow implementations as they leave the legacy page (#624 voice-page-modularity).
// The named owners precede voice.js so a forwarding wrapper never hides the implementation a guard
// means to inspect. A missing owner is allowed during the migration; a missing declaration is not.

pub fn page_source(owners: &[&str]) -> String {
    fn append(path: &std::path::Path, text: &mut String) {
        if path.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .unwrap_or_else(|error| panic!("could not list {}: {error}", path.display()))
                .map(|entry| entry.expect("could not read a page source entry").path())
                .collect();
            entries.sort();
            for entry in entries {
                append(&entry, text);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "ts" || extension == "js" || extension == "mjs")
        {
            text.push_str(
                &std::fs::read_to_string(path)
                    .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display())),
            );
            text.push('\n');
        }
    }

    let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    let mut text = String::new();
    for owner in owners {
        let path = web.join(owner);
        if path.exists() {
            append(&path, &mut text);
        }
    }
    append(&web.join("voice.js"), &mut text);
    text
}
