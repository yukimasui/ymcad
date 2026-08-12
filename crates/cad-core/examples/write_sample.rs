//! 検証用のサンプル図面を書き出す。
//!
//! `tools/validate_ymc.py` と `tools/validate_dxf_r12.py` に食わせるための
//! ファイルを作る。**Rust 側のラウンドトリップテストとは別のロジック**で
//! 検査させるのが目的なので、書き出しは本番と同じ経路を通す。
//!
//! ```sh
//! cargo run -p cad-core --example write_sample -- out.ymc
//! python3 tools/validate_ymc.py out.ymc --expect line=1,circle=1,arc=1,xline=1,polyline=1
//! ```
//!
//! 拡張子が `.dxf` なら DXF R12 として書き出す。

use std::path::PathBuf;
use std::process::ExitCode;

use cad_core::command::{
    AddEntities, AddLayer, CreateGroup, DefineComponent, InsertInstance, SetLayerProperties,
};
use cad_core::component::Placement;
use cad_core::geom::{Arc, Circle, Line, Point2, Polyline, Vec2, Xline};
use cad_core::layer::LineType;
use cad_core::{AciColor, ColorSpec, Document, Entity, EntityId, Geometry, Instance, LayerId};

fn main() -> ExitCode {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("使い方: write_sample <出力パス（.ymc または .dxf）>");
        return ExitCode::FAILURE;
    };
    let path = PathBuf::from(arg);

    let doc = match build() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("図面を組み立てられませんでした: {e}");
            return ExitCode::FAILURE;
        }
    };

    let is_dxf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dxf"));

    let result = if is_dxf {
        cad_core::dxf::write::write_to_file(&doc, &path).map(|warnings| {
            for w in warnings {
                eprintln!("警告: {w}");
            }
        })
    } else {
        cad_core::native::write::write_to_file(&doc, &path)
    };

    match result {
        Ok(()) => {
            println!("書き出しました: {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("書き出しに失敗しました: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 全変種・全属性・グループ・日本語名を含む図面。
fn build() -> cad_core::Result<Document> {
    let mut doc = Document::new();

    doc.apply(Box::new(
        SetLayerProperties::new(LayerId::ZERO).linetype(LineType::Center),
    ))?;

    doc.apply(Box::new(AddLayer::new("壁 outer".to_owned(), AciColor(1))))?;
    let walls = doc
        .layers()
        .by_name("壁 outer")
        .ok_or(cad_core::CadError::LayerNotFound)?;

    doc.apply(Box::new(AddLayer::new("補助線".to_owned(), AciColor(5))))?;
    let helper = doc
        .layers()
        .by_name("補助線")
        .ok_or(cad_core::CadError::LayerNotFound)?;
    doc.apply(Box::new(
        SetLayerProperties::new(helper)
            .visible(false)
            .locked(true)
            .linetype(LineType::Hidden),
    ))?;

    let xline = Xline::new(Point2::new(1.0, 2.0), Vec2::new(3.0, 4.0))
        .ok_or(cad_core::CadError::DegenerateGeometry("作図線"))?;

    let mut circle = Entity::new(
        Geometry::Circle(Circle::new(Point2::new(5.0, 5.0), 2.5)),
        walls,
    );
    circle.color = ColorSpec::Aci(AciColor(4));

    doc.apply(Box::new(AddEntities::many(
        "SAMPLE",
        vec![
            Entity::new(
                Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0))),
                LayerId::ZERO,
            ),
            circle,
            Entity::new(
                Geometry::Arc(Arc::new(Point2::new(1.0, 1.0), 3.0, 0.25, 2.75)),
                walls,
            ),
            Entity::new(Geometry::Xline(xline), helper),
            Entity::new(
                Geometry::Polyline(Polyline::new(
                    vec![
                        Point2::new(0.0, 0.0),
                        Point2::new(1.0, 2.0),
                        Point2::new(3.0, 1.0),
                    ],
                    true,
                )),
                LayerId::ZERO,
            ),
        ],
    )))?;

    // 先頭 2 つをグループにする。
    let ids: Vec<EntityId> = doc.entities().ids().take(2).collect();
    doc.apply(Box::new(CreateGroup::new("GROUP", "外周 ring", ids)))?;

    // ---- コンポーネント（形式 v2 以降） ----
    //
    // 入れ子・反転・回転・倍率を混ぜて、検証スクリプトに一通り通させる。
    let inner_contents = vec![
        Entity::new(
            Geometry::Line(Line::new(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0))),
            walls,
        ),
        Entity::new(
            Geometry::Circle(Circle::new(Point2::new(2.0, 0.0), 1.0)),
            LayerId::ZERO,
        ),
    ];
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "内部品",
        Point2::new(1.0, 0.0),
        inner_contents,
    )))?;
    let inner = doc
        .definitions()
        .by_name("内部品")
        .ok_or(cad_core::CadError::DefinitionNotFound)?;

    let nested = Entity::new(
        Geometry::Instance(Instance::new(
            inner,
            Placement::new(Point2::new(8.0, 0.0), 0.5, 2.0, true)
                .map_err(|_| cad_core::CadError::DegenerateGeometry("配置"))?,
        )),
        LayerId::ZERO,
    );
    doc.apply(Box::new(DefineComponent::new(
        "COMPONENT",
        "外 assembly",
        Point2::ORIGIN,
        vec![nested],
    )))?;
    let outer = doc
        .definitions()
        .by_name("外 assembly")
        .ok_or(cad_core::CadError::DefinitionNotFound)?;

    doc.apply(Box::new(InsertInstance::new(
        "INSERT",
        inner,
        Placement::at(Point2::new(30.0, 0.0)),
        walls,
    )))?;
    doc.apply(Box::new(InsertInstance::new(
        "INSERT",
        outer,
        Placement::new(Point2::new(50.0, 10.0), 1.25, 0.5, true)
            .map_err(|_| cad_core::CadError::DegenerateGeometry("配置"))?,
        LayerId::ZERO,
    )))?;

    Ok(doc)
}
