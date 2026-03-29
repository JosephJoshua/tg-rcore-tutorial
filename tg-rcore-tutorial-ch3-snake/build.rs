use serde::Deserialize;
use std::{collections::HashMap, env, fs, path::PathBuf, process::Command};

const TARGET_ARCH: &str = "riscv64gc-unknown-none-elf";

#[derive(Deserialize, Default)]
struct Cases {
    base: Option<u64>,
    step: Option<u64>,
    cases: Option<Vec<String>>,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LOG");
    println!("cargo:rerun-if-env-changed=TG_USER_DIR");
    println!("cargo:rerun-if-env-changed=TG_USER_VERSION");
    println!("cargo:rerun-if-env-changed=TG_USER_CRATE");
    println!("cargo:rerun-if-env-changed=TG_USER_LOCAL_DIR");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_INTERRUPT");

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_arch == "riscv64" {
        write_linker();
        if should_skip_build_apps() {
            write_dummy_app_asm();
        } else {
            build_apps();
        }
    }
}

fn should_skip_build_apps() -> bool {
    if env::var_os("TG_SKIP_USER_APPS").is_some() {
        return true;
    }
    is_packaged_build()
}

fn write_linker() {
    let ld = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("linker.ld");
    fs::write(&ld, tg_linker::NOBIOS_SCRIPT).unwrap_or_else(|err| {
        panic!("failed to write linker script to {}: {}", ld.display(), err)
    });
    println!("cargo:rustc-link-arg=-T{}", ld.display());
}

fn is_packaged_build() -> bool {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    manifest_dir.contains("/target/package/")
        || manifest_dir.contains("\\target\\package\\")
}

fn build_apps() {
    let tg_user_root = ensure_tg_user();
    let cases_path = tg_user_root.join("cases.toml");
    println!("cargo:rerun-if-changed={}", cases_path.display());
    println!("cargo:rerun-if-changed={}", tg_user_root.join("Cargo.toml").display());
    println!("cargo:rerun-if-changed={}", tg_user_root.join("src").display());

    let cfg = fs::read_to_string(&cases_path).unwrap_or_else(|err| {
        panic!("failed to read cases.toml from {}: {}", cases_path.display(), err)
    });
    let mut cases_map: HashMap<String, Cases> = toml::from_str(&cfg).unwrap_or_else(|err| {
        panic!("failed to parse cases.toml: {err}")
    });

    let case_key = if env::var("CARGO_FEATURE_INTERRUPT").is_ok() {
        "ch3_snake_interrupt"
    } else {
        "ch3_snake_poll"
    };
    let cases = cases_map.remove(case_key).unwrap_or_default();
    let base = cases.base.unwrap_or(0);
    let step = cases.step.unwrap_or(0);
    let names = cases.cases.unwrap_or_default();

    if names.is_empty() {
        panic!("no user cases found for {case_key} in {}", cases_path.display());
    }

    let target_dir = tg_user_root.join("target").join(TARGET_ARCH).join("debug");
    let mut bins: Vec<PathBuf> = Vec::with_capacity(names.len());

    for (i, name) in names.iter().enumerate() {
        let base_address = base + i as u64 * step;
        build_user_app(&tg_user_root, name, base_address);
        let elf = target_dir.join(name);
        let app_path = if base_address != 0 {
            objcopy_to_bin(&elf)
        } else {
            elf
        };
        bins.push(app_path);
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let app_asm = out_dir.join("app.asm");
    write_app_asm(&app_asm, base, step, &bins);
    println!("cargo:rustc-env=APP_ASM={}", app_asm.display());
}

fn build_user_app(tg_user_root: &PathBuf, name: &str, base_address: u64) {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--manifest-path",
        tg_user_root.join("Cargo.toml").to_string_lossy().as_ref(),
        "--bin",
        name,
        "--target",
        TARGET_ARCH,
        "--features",
        "snake",
    ]);

    if base_address != 0 {
        cmd.env("BASE_ADDRESS", base_address.to_string());
    }

    let status = cmd.status().expect("failed to execute cargo build for user app");
    if !status.success() {
        panic!("failed to build user app {name}");
    }
}

fn objcopy_to_bin(elf: &PathBuf) -> PathBuf {
    let bin = elf.with_extension("bin");
    let status = Command::new("rust-objcopy")
        .args([
            elf.to_string_lossy().as_ref(),
            "--strip-all",
            "-O",
            "binary",
            bin.to_string_lossy().as_ref(),
        ])
        .status()
        .expect("failed to execute rust-objcopy");
    if !status.success() {
        panic!("rust-objcopy failed for {}", elf.display());
    }
    bin
}

fn write_app_asm(path: &PathBuf, base: u64, step: u64, bins: &[PathBuf]) {
    use std::io::Write;
    let mut asm = fs::File::create(path)
        .unwrap_or_else(|err| panic!("failed to create {}: {}", path.display(), err));

    writeln!(
        asm,
        "\
.global apps
.section .data
.align 3
apps:
    .quad {base:#x}
    .quad {step:#x}
    .quad {}",
        bins.len()
    )
    .unwrap();

    for (i, _bin) in bins.iter().enumerate() {
        writeln!(asm, "    .quad app_{i}_start").unwrap();
    }
    writeln!(asm, "    .quad app_{}_end", bins.len() - 1).unwrap();

    for (i, bin) in bins.iter().enumerate() {
        writeln!(
            asm,
            "\
app_{i}_start:
    .incbin \"{}\"",
            bin.display()
        )
        .unwrap();
    }
    writeln!(asm, "app_{}_end:", bins.len() - 1).unwrap();
}

fn write_dummy_app_asm() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let app_asm = out_dir.join("app.asm");
    fs::write(
        &app_asm,
        "\
.global apps
.section .data
.align 3
apps:
    .quad 0
    .quad 0
    .quad 0
    .quad dummy_start
    .quad dummy_start
dummy_start:
",
    )
    .unwrap();
    println!("cargo:rustc-env=APP_ASM={}", app_asm.display());
}

fn ensure_tg_user() -> PathBuf {
    if let Ok(dir) = env::var("TG_USER_DIR") {
        let path = PathBuf::from(dir);
        if path.exists() {
            return path;
        }
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    if let Ok(local_dir) = env::var("TG_USER_LOCAL_DIR") {
        let local_path = manifest_dir.join(&local_dir);
        if local_path.exists() {
            return local_path;
        }
    }

    let crate_name = env::var("TG_USER_CRATE").unwrap_or_else(|_| "tg-rcore-tutorial-user".into());
    let version = env::var("TG_USER_VERSION").unwrap_or_else(|_| "0.4.8".into());
    let local_dir = env::var("TG_USER_LOCAL_DIR").unwrap_or_else(|_| "tg-user".into());
    let dest = manifest_dir.join(&local_dir);

    eprintln!("Cloning {crate_name}@{version} from crates.io into {}", dest.display());
    let status = Command::new("cargo")
        .args(["clone", &format!("{crate_name}@{version}"), "--directory"])
        .arg(&dest)
        .status()
        .expect("failed to run cargo clone — install it with: cargo install cargo-clone");
    if !status.success() {
        panic!("cargo clone failed for {crate_name}@{version}");
    }

    let cargo_toml_path = dest.join("Cargo.toml");
    let mut content = fs::read_to_string(&cargo_toml_path).unwrap();
    if !content.contains("[workspace]") {
        content.push_str("\n[workspace]\n");
        fs::write(&cargo_toml_path, content).unwrap();
    }

    dest
}
