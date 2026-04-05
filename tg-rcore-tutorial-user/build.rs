fn main() {
    use std::{env, fs, path::PathBuf};

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LOG");
    println!("cargo:rerun-if-env-changed=BASE_ADDRESS");
    println!("cargo:rerun-if-env-changed=CHAPTER");

    if let Ok(chapter) = env::var("CHAPTER") {
        println!("cargo:rustc-env=CHAPTER={chapter}");
    }

    if let Some(base) = env::var("BASE_ADDRESS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        let text = format!(
            "\
OUTPUT_ARCH(riscv)
ENTRY(_start)
SECTIONS {{
    . = {base};
    .text : {{
        *(.text.entry)
        *(.text .text.*)
    }}
    .rodata : {{
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }}
    .data : {{
        *(.data .data.*)
        *(.sdata .sdata.*)
    }}
    .bss : {{
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }}
}}"
        );
        let ld = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("linker.ld");
        fs::write(&ld, text).unwrap();
        println!("cargo:rustc-link-arg=-T{}", ld.display());
    }

    // Compile doomgeneric C sources when doom feature is enabled
    #[cfg(feature = "doom")]
    {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let doom_dir = PathBuf::from(&manifest_dir).join("doomgeneric");
        if doom_dir.exists() {
            println!("cargo:rerun-if-changed=doomgeneric/");

            let mut build = cc::Build::new();
            build
                .target("riscv64gc-unknown-none-elf")
                .opt_level(2)
                .flag("-ffreestanding")
                .flag("-nostdlib")
                .flag("-DNORMALUNIX")
                .flag("-DSNDSERV")
                .flag("-D_DEFAULT_SOURCE")
                .flag("-mabi=lp64d")
                .flag("-fno-builtin")
                .warnings(false)
                .include(doom_dir.join("include"))
                .include(&doom_dir);

            // Only include core engine files + our stubs. Exclude all
            // platform backends (SDL, Allegro, Xlib, Win, Emscripten)
            // and system files we stub ourselves.
            for entry in fs::read_dir(&doom_dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "c") {
                    let fname = path.file_name().unwrap().to_str().unwrap();
                    // Skip platform backends and system files we replace
                    if fname.starts_with("doomgeneric_") // platform backends
                        || fname == "i_main.c"
                        || fname == "i_sdlmusic.c"
                        || fname == "i_sdlsound.c"
                        || fname == "i_allegrosound.c"
                        || fname == "i_allegromusic.c"
                        || fname == "i_sound.c"
                        || fname == "i_cdmus.c"
                        || fname == "i_input.c"
                        || fname == "i_joystick.c"
                        || fname == "i_timer.c"
                        || fname == "i_system.c"
                    {
                        continue;
                    }
                    build.file(&path);
                }
            }

            build.compile("doomgeneric");
        }
    }
}
