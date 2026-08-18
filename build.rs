use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=native/macos/coordinated_access.h");
    println!("cargo:rerun-if-changed=native/macos/coordinated_access.m");
    for variable in ["CC", "AR", "MACOSX_DEPLOYMENT_TARGET"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let target = env::var("TARGET").expect("Cargo sets TARGET");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let object = out_dir.join("coordinated_access.o");
    let archive = out_dir.join("libzwirn_macos_access.a");
    let compiler = target_tool("CC", &target).unwrap_or_else(|| OsString::from("cc"));
    let archiver = target_tool("AR", &target).unwrap_or_else(|| OsString::from("ar"));
    let deployment_target = deployment_target(&target);

    let mut compile = Command::new(compiler);
    compile.args([
        "-std=c11",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-fblocks",
        "-fno-objc-arc",
        "-fobjc-exceptions",
        "-c",
        "native/macos/coordinated_access.m",
        "-o",
    ]);
    compile.arg(&object);
    compile.arg(format!("-mmacosx-version-min={deployment_target}"));
    match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => {
            compile.args(["-arch", "arm64"]);
        }
        Ok("x86_64") => {
            compile.args(["-arch", "x86_64"]);
        }
        _ => {}
    }
    run(&mut compile, "compile the macOS coordination bridge");

    let mut archive_command = Command::new(archiver);
    archive_command.arg("crs").arg(&archive).arg(&object);
    run(
        &mut archive_command,
        "archive the macOS coordination bridge",
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=zwirn_macos_access");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}

fn target_tool(name: &str, target: &str) -> Option<OsString> {
    let underscored_target = target.replace('-', "_");
    let variables = [
        format!("{name}_{target}"),
        format!("{name}_{underscored_target}"),
        format!("TARGET_{name}"),
        name.to_owned(),
    ];
    for variable in &variables {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    variables.into_iter().find_map(env::var_os)
}

fn deployment_target(target: &str) -> String {
    if let Ok(version) = env::var("MACOSX_DEPLOYMENT_TARGET") {
        return version;
    }

    let queried = env::var_os("RUSTC").and_then(|rustc| {
        Command::new(rustc)
            .args(["--print", "deployment-target", "--target", target])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|output| {
                output
                    .trim()
                    .strip_prefix("MACOSX_DEPLOYMENT_TARGET=")
                    .map(str::to_owned)
            })
    });
    queried.unwrap_or_else(|| match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "11.0".to_owned(),
        Ok("x86_64") => "10.12".to_owned(),
        _ => panic!("cannot determine the macOS deployment target for {target}"),
    })
}

fn run(command: &mut Command, description: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("cannot {description}: {error}"));
    assert!(status.success(), "failed to {description}: {status}");
}
