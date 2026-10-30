// Запрет консольного окна в релизной сборке Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    biogit_app_lib::run()
}
