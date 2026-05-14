fn main() -> eframe::Result<()> {
    csvfastview::app::run(std::env::args().nth(1).map(Into::into))
}
