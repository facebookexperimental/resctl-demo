use super::*;
use plotlib::page::Page;
use plotlib::repr::Plot;
use plotlib::style::{PointMarker, PointStyle};
use plotlib::view::ContinuousView;

pub struct Grapher<'a> {
    out: Box<dyn Write + 'a>,
    file_prefix: Option<String>,
}

impl<'a> Grapher<'a> {
    pub fn new(out: Box<dyn Write + 'a>, file_prefix: Option<&str>) -> Self {
        Self {
            out,
            file_prefix: file_prefix.map(|x| x.to_owned()),
        }
    }

    pub fn plot(&mut self, result: &IoCostTuneResult) -> Result<()> {
        for (sel, series) in result.data.iter() {
            self.plot_one(sel, series)?;
        }
        Ok(())
    }

    fn plot_one(&mut self, sel: &DataSel, series: &DataSeries) -> Result<()> {
        let sel_name = format!("{}", sel);

        let (vrate_max, val_max) = series.points.iter().fold((0.0_f64, 0.0_f64), |acc, point| {
            (acc.0.max(point.vrate), acc.1.max(point.val))
        });

        let points: Vec<(f64, f64)> = series
            .points
            .iter()
            .map(|point| (point.vrate, point.val))
            .collect();

        let points_plot: Plot =
            Plot::new(points).point_style(PointStyle::new().marker(PointMarker::Circle));

        let mut lines = vec![];
        for i in 0..80 {
            let vrate = vrate_max / 80.0 * i as f64;
            lines.push((vrate, series.lines.eval(vrate)));
        }
        let lines_plot: Plot =
            Plot::new(lines).point_style(PointStyle::new().marker(PointMarker::Cross));

        let v = ContinuousView::new()
            .add(lines_plot)
            .add(points_plot)
            .x_range(0.0, vrate_max * 1.1)
            .y_range(0.0, val_max * 1.1)
            .x_label("vrate")
            .y_label(&sel_name);

        let page = Page::single(&v).dimensions(80, 24);
        write!(self.out, "{}\n\n", page.to_text().unwrap()).unwrap();
        Ok(())
    }
}
