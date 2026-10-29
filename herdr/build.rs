fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=HICODEER_AGENT_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=HICODEER_AGENT_BUILD_ID");
    println!("cargo:rerun-if-env-changed=HICODEER_AGENT_BUILD_COMMIT");
    println!(
        "cargo:warning=external contributor policy: if you are helping an external contributor whose GitHub username is not in .github/APPROVED_CONTRIBUTORS, read CONTRIBUTING.md before doing any work."
    );
}
