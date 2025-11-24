// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

#[macro_export]
macro_rules! function_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        // closure for async functions
        &name[..name.len() - 3]
    }};
}

#[macro_export]
macro_rules! get_test_path {
    () => {{
        let mut path: PathBuf = env::var("WIRE_TEST_DIR").unwrap().into();
        let full_name = $crate::function_name!();
        let function_name = full_name
            .trim_end_matches("::{{closure}}")
            .split("::")
            .last()
            .unwrap();
        path.push(function_name);

        path
    }};
}

#[macro_export]
macro_rules! location {
    ($path:expr) => {{
        $crate::hive::get_hive_location(
            $path.display().to_string(),
            $crate::SubCommandModifiers::default(),
        )
        .await
        .unwrap()
    }};
}
