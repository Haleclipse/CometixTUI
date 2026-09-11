//! Maps to chalk's examples/screenshot.js + examples/rainbow.js: renders every
//! style group so the full surface is visible at a glance.
//!
//! ```sh
//! cargo run -p cometix-chalk --example showcase
//! ```

use chalk::{Chalk, NamedColor};

fn main() {
    let c = Chalk::new;

    println!();
    println!(
        "  {} {} {} {} {} {} {}",
        c().bold().apply("bold"),
        c().dim().apply("dim"),
        c().italic().apply("italic"),
        c().underline().apply("underline"),
        c().inverse().apply("inverse"),
        c().strikethrough().apply("strikethrough"),
        c().visible().apply("visible"),
    );
    println!(
        "  {} {} {} {}",
        c().underline_double().apply("double"),
        c().underline_curly().apply("curly"),
        c().underline_dotted().apply("dotted"),
        c().underline_dashed().apply("dashed"),
    );

    let named = [
        NamedColor::Black,
        NamedColor::Red,
        NamedColor::Green,
        NamedColor::Yellow,
        NamedColor::Blue,
        NamedColor::Magenta,
        NamedColor::Cyan,
        NamedColor::White,
    ];
    print!("  ");
    for color in named {
        print!("{} ", c().color(color).apply(&format!("{color:?}")));
    }
    println!();
    print!("  ");
    for color in named {
        print!("{} ", c().bg_color(color).apply(&format!("{color:?}")));
    }
    println!();

    println!(
        "  {} {} {}",
        c().rgb(215, 119, 87).apply("rgb(215,119,87)"),
        c().hex("#d77757").apply("#d77757"),
        c().ansi256(174).apply("ansi256(174)"),
    );
    println!(
        "  {} {}",
        c().underline_rgb(255, 0, 0).apply("underline rgb"),
        c().red()
            .underline_color(NamedColor::Red)
            .underline_curly()
            .apply("red curly"),
    );

    // Maps to examples/rainbow.js, statically: a hue sweep over truecolor.
    print!("  ");
    let text = "We hope you enjoy Chalk! <3";
    let step = 360.0 / text.chars().filter(|c| !c.is_whitespace()).count() as f64;
    let mut hue = 0.0f64;
    for ch in text.chars() {
        if ch.is_whitespace() {
            print!("{ch}");
            continue;
        }
        let (r, g, b) = hsl_to_rgb(hue, 1.0, 0.5);
        print!("{}", c().rgb(r, g, b).apply(&ch.to_string()));
        hue = (hue + step) % 360.0;
    }
    println!();
    println!();
    println!("  stdout color level: {}", chalk::stdout_level());
}

/// Minimal HSL→RGB for the rainbow sweep.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}
