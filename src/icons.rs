//! Toolbar icons, drawn with cairo on a 20×20 grid so they do not depend on
//! whichever icon theme is installed.

use crate::model::{draw_arrow, Pt, Tool};
use gtk4::cairo::{self, Context};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Tool(Tool),
    Undo,
    Redo,
    Save,
    Copy,
    Close,
}

pub const SIZE: i32 = 20;

fn line(cr: &Context, pts: &[(f64, f64)]) {
    if let Some(&(x, y)) = pts.first() {
        cr.move_to(x, y);
        for &(x, y) in &pts[1..] {
            cr.line_to(x, y);
        }
    }
}

pub fn draw(cr: &Context, icon: Icon) {
    let _ = cr.save();
    cr.set_source_rgb(0.93, 0.93, 0.93);
    cr.set_line_width(1.7);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    match icon {
        Icon::Tool(Tool::Select) => {
            line(
                cr,
                &[
                    (5.0, 2.5),
                    (5.0, 16.0),
                    (8.4, 12.9),
                    (10.8, 17.8),
                    (13.0, 16.8),
                    (10.7, 12.0),
                    (15.2, 12.0),
                ],
            );
            cr.close_path();
            let _ = cr.fill();
        }
        Icon::Tool(Tool::Arrow) => draw_arrow(cr, Pt::new(4.0, 16.0), Pt::new(16.5, 3.5), 1.8),
        Icon::Tool(Tool::Line) => {
            line(cr, &[(4.0, 16.0), (16.0, 4.0)]);
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Rect) => {
            cr.rectangle(3.5, 5.5, 13.0, 9.0);
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Ellipse) => {
            let _ = cr.save();
            cr.translate(10.0, 10.0);
            cr.scale(7.0, 5.0);
            cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
            let _ = cr.restore();
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Pen) => {
            cr.move_to(3.0, 14.0);
            cr.curve_to(6.0, 3.0, 9.0, 19.0, 12.0, 9.0);
            cr.curve_to(13.0, 6.0, 15.5, 6.0, 17.0, 8.5);
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Marker) => {
            cr.set_source_rgba(1.0, 0.82, 0.15, 0.95);
            cr.set_line_width(6.0);
            cr.set_line_cap(cairo::LineCap::Butt);
            line(cr, &[(3.5, 13.0), (16.5, 7.0)]);
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Text) => {
            cr.set_line_width(2.0);
            line(cr, &[(4.5, 4.5), (15.5, 4.5)]);
            line(cr, &[(10.0, 4.5), (10.0, 16.0)]);
            line(cr, &[(8.0, 16.0), (12.0, 16.0)]);
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Number) => {
            cr.arc(10.0, 10.0, 7.5, 0.0, 2.0 * PI);
            let _ = cr.fill();
            cr.set_source_rgb(0.13, 0.13, 0.15);
            line(cr, &[(8.3, 8.0), (10.3, 6.2), (10.3, 14.0)]);
            let _ = cr.stroke();
        }
        Icon::Tool(Tool::Blur) => {
            for row in 0..4 {
                for col in 0..4 {
                    let alpha = if (row + col) % 2 == 0 { 0.95 } else { 0.35 };
                    cr.set_source_rgba(0.93, 0.93, 0.93, alpha);
                    cr.rectangle(3.0 + col as f64 * 3.5, 3.0 + row as f64 * 3.5, 3.5, 3.5);
                    let _ = cr.fill();
                }
            }
        }
        Icon::Undo | Icon::Redo => {
            if icon == Icon::Redo {
                cr.translate(SIZE as f64, 0.0);
                cr.scale(-1.0, 1.0);
            }
            cr.arc(10.5, 11.0, 5.5, -PI * 0.9, PI * 0.5);
            let _ = cr.stroke();
            line(cr, &[(4.6, 4.6), (5.2, 9.3), (9.6, 7.9)]);
            let _ = cr.stroke();
        }
        Icon::Save => {
            line(cr, &[(10.0, 3.0), (10.0, 12.0)]);
            line(cr, &[(6.5, 8.5), (10.0, 12.0), (13.5, 8.5)]);
            line(cr, &[(4.0, 13.0), (4.0, 16.5), (16.0, 16.5), (16.0, 13.0)]);
            let _ = cr.stroke();
        }
        Icon::Copy => {
            cr.rectangle(7.5, 7.5, 9.0, 9.0);
            line(cr, &[(4.0, 12.5), (4.0, 3.5), (12.5, 3.5)]);
            let _ = cr.stroke();
        }
        Icon::Close => {
            cr.set_line_width(2.0);
            line(cr, &[(5.0, 5.0), (15.0, 15.0)]);
            line(cr, &[(15.0, 5.0), (5.0, 15.0)]);
            let _ = cr.stroke();
        }
    }
    let _ = cr.restore();
}
