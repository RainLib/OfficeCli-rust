use crate::{
    hash_bytes, Bundle, HcdError, HcdManifest, MAX_CHUNK_BYTES, MAX_CONTROL_PART_BYTES,
    MAX_REVISION,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;

pub const DEFAULT_HTML_PRESENTATION_MAX_BYTES: u64 = 64 * 1024 * 1024;

const GRID_PRESENTATION_SHELL_CSS: &str = r#"
body[data-hcd-profile="grid"]{width:100%;max-width:none;min-width:0;height:100vh;margin:0;padding:0;display:flex;flex-direction:column;overflow:hidden;background:#f0f0f0;font:13px "Segoe UI",-apple-system,BlinkMacSystemFont,"Microsoft YaHei",Arial,sans-serif;color:#333}
.hcd-grid-title{flex:none;padding:12px 20px;background:#217346;color:#fff;font-size:14px;font-weight:600;line-height:17px;box-sizing:border-box}
.hcd-grid-workspace{flex:1;min-height:0;overflow:auto;background:#fff;padding:0}.hcd-grid-sheet-content{display:none;position:relative;min-width:100%;min-height:100%;width:max-content;background:#fff;box-sizing:border-box}.hcd-grid-sheet-content[data-active="true"]{display:block}.hcd-grid-sheet-content>[data-hcd-sheet]{margin:0;box-shadow:none}.hcd-grid-canvas{position:absolute;z-index:0;left:0;top:0;pointer-events:none}
.hcd-grid-tabs{display:flex;flex:none;align-items:stretch;min-height:35px;padding:0 8px;border-top:1px solid #ccc;background:#e0e0e0;overflow-x:auto;box-sizing:border-box}.hcd-grid-tab{--tab-color:#e8e8e8;appearance:none;border:1px solid #bbb;border-top:0;border-bottom:0;background:var(--tab-color);padding:8px 16px;color:#333;font:12px "Segoe UI",-apple-system,BlinkMacSystemFont,sans-serif;white-space:nowrap;cursor:pointer}.hcd-grid-tab+.hcd-grid-tab{margin-left:-1px}.hcd-grid-tab:hover{filter:brightness(.97)}.hcd-grid-tab[aria-selected="true"]{background:#fff;border-bottom:3px solid #217346;font-weight:600}.hcd-grid-tab:focus-visible{outline:2px solid #107c41;outline-offset:-2px}
.hcd-grid-sheet-content>.hcd-sheet{position:relative;z-index:1;overflow:visible;background:transparent}.hcd-grid-sheet-content>.hcd-sheet-drawing-layer{position:absolute;z-index:2;left:40px;top:24px;overflow:visible;background:transparent;pointer-events:none}.hcd-grid-sheet-content>.hcd-sheet-drawing-layer .hcd-sheet-picture,.hcd-grid-sheet-content>.hcd-sheet-drawing-layer .hcd-sheet-chart{pointer-events:auto}
.hcd-grid{border-collapse:collapse;border-spacing:0;table-layout:fixed;background:#fff;font:11px "Segoe UI",-apple-system,BlinkMacSystemFont,"Microsoft YaHei",Arial,sans-serif}.hcd-grid td{height:20px;padding:2px 4px;overflow:hidden;text-overflow:ellipsis;vertical-align:bottom;box-shadow:inset -1px -1px 0 #e0e0e0}.hcd-grid td>span{white-space:inherit}.hcd-grid .hcd-column-header,.hcd-grid .hcd-row-header,.hcd-grid .hcd-corner-header{box-sizing:border-box;border:1px solid #e0e0e0;background:#f8f8f8;color:#666;font-size:10px;font-weight:400;text-align:center;user-select:none}.hcd-grid .hcd-column-header{position:sticky;top:0;z-index:3;height:24px;min-width:50px;padding:2px 4px}.hcd-grid .hcd-row-header{position:sticky;left:0;z-index:2;min-width:40px;width:40px;padding:2px 4px}.hcd-grid .hcd-corner-header{position:sticky;top:0;left:0;z-index:4;min-width:40px;width:40px}.hcd-grid .hcd-row-header-col{width:40px!important;min-width:40px}
@media print{.hcd-grid-title,.hcd-grid-tabs{display:none!important}body[data-hcd-profile="grid"]{height:auto;background:#fff}.hcd-grid-workspace{overflow:visible}.hcd-grid-sheet-content{display:block!important;min-height:0;break-after:page}}
"#;

const GRID_PRESENTATION_SCRIPT: &str = r#"<script>(()=>{const body=document.body;if(body.dataset.hcdProfile!=="grid")return;const regions=[...body.querySelectorAll("[data-hcd-sheet]")];if(!regions.length)return;const columnName=n=>{let s="";while(n>0){n-=1;s=String.fromCharCode(65+n%26)+s;n=Math.floor(n/26)}return s};for(const section of regions.filter(node=>node.classList.contains("hcd-sheet"))){if(section.dataset.hcdShowRowColumnHeaders==="false")continue;for(const table of section.querySelectorAll("table.hcd-grid")){if(table.tHead)continue;let max=0;for(const cell of table.querySelectorAll("tbody td[data-hcd-column]")){const column=Number(cell.dataset.hcdColumn);const span=Number(cell.getAttribute("colspan")||1);if(Number.isFinite(column))max=Math.max(max,column+span-1)}if(max<1)max=1;const head=table.createTHead();const row=head.insertRow();const corner=document.createElement("th");corner.className="hcd-corner-header";corner.setAttribute("aria-label","Select all");row.append(corner);for(let column=1;column<=max;column++){const th=document.createElement("th");th.className="hcd-column-header";th.scope="col";th.textContent=columnName(column);row.append(th)}const colgroup=table.querySelector(":scope > colgroup");if(colgroup&&!colgroup.querySelector("col[data-hcd-column-start]")){for(let column=1;column<=max;column++){const col=document.createElement("col");col.style.width="64px";colgroup.append(col)}}if(colgroup&&!colgroup.querySelector(".hcd-row-header-col")){const col=document.createElement("col");col.className="hcd-row-header-col";colgroup.prepend(col)}for(const dataRow of table.tBodies[0]?.rows||[]){if(dataRow.querySelector(":scope > .hcd-row-header"))continue;const th=document.createElement("th");th.className="hcd-row-header";th.scope="row";th.textContent=dataRow.dataset.hcdRow||"";dataRow.prepend(th)}}}const sheets=[];for(const region of regions){const name=region.dataset.hcdSheet||"Sheet";let sheet=sheets.find(item=>item.name===name);if(!sheet){sheet={name,index:Number(region.dataset.hcdSheetIndex||sheets.length),state:region.dataset.hcdSheetState||"visible",regions:[]};sheets.push(sheet)}sheet.regions.push(region)}sheets.sort((a,b)=>a.index-b.index);const visible=sheets.filter(sheet=>sheet.state==="visible");const selectable=visible.length?visible:sheets;const title=document.createElement("header");title.className="hcd-grid-title";title.textContent="Workbook Preview";const workspace=document.createElement("main");workspace.className="hcd-grid-workspace";body.insertBefore(title,regions[0]);body.insertBefore(workspace,regions[0]);for(const sheet of sheets){const content=document.createElement("section");content.className="hcd-grid-sheet-content";content.dataset.sheet=sheet.name;content.dataset.sheetIndex=String(sheet.index);let width=0,height=0;for(const region of sheet.regions){content.append(region);if(region.classList.contains("hcd-sheet-drawing-layer")){width=Math.max(width,parseFloat(region.style.width)||0);height=Math.max(height,parseFloat(region.style.height)||0)}}if(width)content.style.minWidth=Math.ceil(width+40)+"px";if(height)content.style.minHeight=Math.ceil(height+24)+"px";sheet.content=content;workspace.append(content)}const tabs=document.createElement("nav");tabs.className="hcd-grid-tabs";tabs.setAttribute("role","tablist");tabs.setAttribute("aria-label","Workbook sheets");body.append(tabs);const activate=name=>{for(const sheet of sheets)sheet.content.dataset.active=String(sheet.name===name);for(const button of tabs.querySelectorAll("button"))button.setAttribute("aria-selected",String(button.dataset.sheet===name));workspace.scrollTo(0,0)};for(const sheet of selectable){const button=document.createElement("button");button.type="button";button.className="hcd-grid-tab";button.dataset.sheet=sheet.name;button.setAttribute("role","tab");button.textContent=sheet.name;button.addEventListener("click",()=>activate(sheet.name));tabs.append(button)}activate(selectable[0].name)})();</script>"#;

// A worksheet drawing anchor is expressed in cell coordinates, not in a
// fixed-pixel canvas. Recompute visual geometry from the presented column and
// row metrics so custom dimensions and the row/column header gutter cannot
// shift pictures or charts away from their source cells.
const GRID_GEOMETRY_SCRIPT: &str = r##"<script>(()=>{
const MAX_COLUMNS=16384,MAX_ROWS=1048576,MAX_DPR=2,MAX_VISIBLE_LINES=2048,EMU_PER_PX=9525,DEFAULT_COLUMN_EMU=609600,DEFAULT_ROW_EMU=190500;
const columnNumber=value=>{let result=0;for(const character of value)result=result*26+character.charCodeAt(0)-64;return result};
const parseCell=value=>{const match=/^([A-Z]+)([1-9][0-9]*)$/.exec(value||"");return match?{column:columnNumber(match[1]),row:Number(match[2])}:null};
const columnName=value=>{let result="";while(value>0){value--;result=String.fromCharCode(65+value%26)+result;value=Math.floor(value/26)}return result};
const upperBound=(values,wanted)=>{let low=0,high=values.length;while(low<high){const middle=(low+high)>>>1;if(values[middle]<=wanted)low=middle+1;else high=middle}return low};
const workspace=document.querySelector(".hcd-grid-workspace");if(!workspace)return;
const renderers=[];
for(const content of document.querySelectorAll(".hcd-grid-sheet-content")){
  const visuals=[...content.querySelectorAll(".hcd-sheet-picture[data-hcd-anchor-from],.hcd-sheet-chart[data-hcd-anchor-from]")];
  const gridSections=[...content.querySelectorAll(":scope > .hcd-sheet")];
  const tables=gridSections.map(section=>section.querySelector("table.hcd-grid")).filter(Boolean);
  const gridSection=gridSections[0],table=tables[0];if(!table)continue;
  let dataColumns=1,dataRows=1,visualColumns=1,visualRows=1;
  for(const cell of content.querySelectorAll(".hcd-grid tbody td[data-hcd-column]")){const column=Number(cell.dataset.hcdColumn),span=Number(cell.getAttribute("colspan")||1);if(Number.isFinite(column))dataColumns=Math.max(dataColumns,column+span-1)}
  for(const row of content.querySelectorAll(".hcd-grid tbody tr[data-hcd-row]")){const number=Number(row.dataset.hcdRow);if(Number.isFinite(number))dataRows=Math.max(dataRows,number)}
  for(const visual of visuals){for(const reference of [visual.dataset.hcdAnchorFrom,visual.dataset.hcdAnchorTo]){const marker=parseCell(reference);if(marker){visualColumns=Math.max(visualColumns,marker.column);visualRows=Math.max(visualRows,marker.row)}}}
  const targetColumns=Math.min(MAX_COLUMNS,Math.max(dataColumns,visualColumns));
  const targetRows=Math.min(MAX_ROWS,Math.max(dataRows,visualRows));
  const sourceColumns=[...table.querySelectorAll(":scope > colgroup > col[data-hcd-column-start]")].map(column=>({start:Number(column.dataset.hcdColumnStart),end:Number(column.dataset.hcdColumnEnd),width:Number(column.dataset.hcdWidth),hidden:column.dataset.hcdHidden==="true"})).sort((left,right)=>left.start-right.start);
  const defaultWidth=Math.max(1,(Number(gridSection.dataset.hcdDefaultColumnWidth)||64/7.5)*7.5);
  const widths=Array(targetColumns+1).fill(defaultWidth);
  let sourceColumnIndex=0;
  for(let column=1;column<=targetColumns;column++){
    while(sourceColumnIndex+1<sourceColumns.length&&sourceColumns[sourceColumnIndex].end<column)sourceColumnIndex++;
    const candidate=sourceColumns[sourceColumnIndex],source=candidate&&candidate.start<=column&&column<=candidate.end?candidate:null;
    widths[column]=source?.hidden?0:Math.max(1,(Number.isFinite(source?.width)?source.width*7.5:defaultWidth));
  }
  for(const currentTable of tables){const colgroup=currentTable.querySelector(":scope > colgroup")||currentTable.insertBefore(document.createElement("colgroup"),currentTable.firstChild);colgroup.replaceChildren();const gutter=document.createElement("col");gutter.className="hcd-row-header-col";colgroup.append(gutter);let start=1;while(start<=dataColumns){const width=widths[start],hidden=width===0;let end=start;while(end+1<=dataColumns&&widths[end+1]===width)end++;const element=document.createElement("col");element.span=end-start+1;if(hidden)element.style.display="none";else element.style.width=width+"px";colgroup.append(element);start=end+1}}
  const columnOffsets=new Float64Array(targetColumns+1);for(let column=1;column<=targetColumns;column++)columnOffsets[column]=columnOffsets[column-1]+widths[column];
  const defaultRowHeight=DEFAULT_ROW_EMU/EMU_PER_PX,rowDeltas=[];
  const rowDeltaMap=new Map();for(const row of content.querySelectorAll(".hcd-grid tbody tr[data-hcd-row]")){const number=Number(row.dataset.hcdRow);if(!Number.isFinite(number)||number>targetRows)continue;let height=defaultRowHeight;if(row.dataset.hcdHidden==="true")height=0;else if(row.dataset.hcdHeightPoints)height=Number(row.dataset.hcdHeightPoints)*96/72;if(height!==defaultRowHeight)rowDeltaMap.set(number,height-defaultRowHeight)}for(const entry of rowDeltaMap)rowDeltas.push(entry);
  rowDeltas.sort((left,right)=>left[0]-right[0]);const rowDeltaNumbers=rowDeltas.map(entry=>entry[0]),rowDeltaPrefix=new Float64Array(rowDeltas.length+1);for(let index=0;index<rowDeltas.length;index++)rowDeltaPrefix[index+1]=rowDeltaPrefix[index]+rowDeltas[index][1];
  const rowOffset=row=>(Math.max(1,row)-1)*defaultRowHeight+rowDeltaPrefix[upperBound(rowDeltaNumbers,row-1)];
  const rowAtOffset=offset=>{let low=1,high=targetRows;while(low<high){const middle=Math.ceil((low+high)/2);if(rowOffset(middle)<=offset)low=middle;else high=middle-1}return low};
  let previousRow=0;for(let sectionIndex=0;sectionIndex<gridSections.length;sectionIndex++){const section=gridSections[sectionIndex],currentTable=tables[sectionIndex];if(!currentTable)continue;if(sectionIndex>0&&currentTable.tHead)currentTable.tHead.hidden=true;let skippedHeight=0,lastRow=previousRow;for(const currentRow of currentTable.querySelectorAll("tbody tr[data-hcd-row]")){const number=Number(currentRow.dataset.hcdRow);if(!Number.isFinite(number)||number<=lastRow)continue;if(number>lastRow+1)skippedHeight+=(number-lastRow-1)*defaultRowHeight;if(skippedHeight)currentRow.style.transform="translateY("+skippedHeight+"px)";lastRow=number}section.style.paddingBottom=skippedHeight+"px";previousRow=Math.max(previousRow,lastRow)}
  let maximumRight=columnOffsets[targetColumns],maximumBottom=rowOffset(targetRows+1);
  for(const visual of visuals){
    const from=parseCell(visual.dataset.hcdAnchorFrom),to=parseCell(visual.dataset.hcdAnchorTo);if(!from)continue;
    const sourceX=Number(visual.dataset.hcdXEmu)||0,sourceY=Number(visual.dataset.hcdYEmu)||0,sourceWidth=Number(visual.dataset.hcdWidthEmu)||0,sourceHeight=Number(visual.dataset.hcdHeightEmu)||0;
    const left=columnOffsets[Math.min(from.column-1,targetColumns)]+(sourceX-(from.column-1)*DEFAULT_COLUMN_EMU)/EMU_PER_PX;
    const top=rowOffset(Math.min(from.row,targetRows+1))+(sourceY-(from.row-1)*DEFAULT_ROW_EMU)/EMU_PER_PX;
    const right=to?columnOffsets[Math.min(to.column-1,targetColumns)]+(sourceX+sourceWidth-(to.column-1)*DEFAULT_COLUMN_EMU)/EMU_PER_PX:left+sourceWidth/EMU_PER_PX;
    const bottom=to?rowOffset(Math.min(to.row,targetRows+1))+(sourceY+sourceHeight-(to.row-1)*DEFAULT_ROW_EMU)/EMU_PER_PX:top+sourceHeight/EMU_PER_PX;
    visual.style.left=Math.max(0,left)+"px";visual.style.top=Math.max(0,top)+"px";visual.style.width=Math.max(1,right-left)+"px";visual.style.height=Math.max(1,bottom-top)+"px";
    maximumRight=Math.max(maximumRight,right);maximumBottom=Math.max(maximumBottom,bottom);
  }
  for(const layer of content.querySelectorAll(":scope > .hcd-sheet-drawing-layer")){layer.style.width=Math.ceil(maximumRight)+"px";layer.style.height=Math.ceil(maximumBottom)+"px"}
  content.style.minWidth=Math.ceil(maximumRight+40)+"px";content.style.minHeight=Math.ceil(maximumBottom+24)+"px";
  const canvas=document.createElement("canvas");canvas.className="hcd-grid-canvas";canvas.setAttribute("aria-hidden","true");content.prepend(canvas);
  const draw=()=>{if(content.dataset.active!=="true")return;const width=workspace.clientWidth,height=workspace.clientHeight,dpr=Math.min(MAX_DPR,window.devicePixelRatio||1);canvas.style.left=workspace.scrollLeft+"px";canvas.style.top=workspace.scrollTop+"px";canvas.style.width=width+"px";canvas.style.height=height+"px";canvas.width=Math.max(1,Math.round(width*dpr));canvas.height=Math.max(1,Math.round(height*dpr));const context=canvas.getContext("2d");if(!context)return;context.setTransform(dpr,0,0,dpr,0,0);context.clearRect(0,0,width,height);context.fillStyle="#fff";context.fillRect(0,0,width,height);context.strokeStyle="#e0e0e0";context.lineWidth=1;const scrollX=workspace.scrollLeft,scrollY=workspace.scrollTop,firstColumn=Math.max(1,upperBound(columnOffsets,Math.max(0,scrollX-40))),lastColumn=Math.min(targetColumns,upperBound(columnOffsets,scrollX+width));context.beginPath();let lines=0;for(let column=firstColumn;column<=lastColumn+1&&lines++<MAX_VISIBLE_LINES;column++){const x=Math.round(40+columnOffsets[column-1]-scrollX)+.5;context.moveTo(x,24);context.lineTo(x,height)}const firstRow=Math.max(1,rowAtOffset(Math.max(0,scrollY-24))),lastRow=Math.min(targetRows,rowAtOffset(scrollY+height)+1);for(let row=firstRow;row<=lastRow+1&&lines++<MAX_VISIBLE_LINES;row++){const y=Math.round(24+rowOffset(row)-scrollY)+.5;context.moveTo(40,y);context.lineTo(width,y)}context.stroke();context.fillStyle="#f8f8f8";context.fillRect(0,0,width,24);context.fillRect(0,0,40,height);context.strokeRect(.5,.5,39,23);context.fillStyle="#666";context.font='10px "Segoe UI",sans-serif';context.textAlign="center";context.textBaseline="middle";for(let column=firstColumn;column<=lastColumn&&column-firstColumn<MAX_VISIBLE_LINES;column++){const left=40+columnOffsets[column-1]-scrollX,right=40+columnOffsets[column]-scrollX;if(right>40&&left<width)context.fillText(columnName(column),(left+right)/2,12)}for(let row=firstRow;row<=lastRow&&row-firstRow<MAX_VISIBLE_LINES;row++){const top=24+rowOffset(row)-scrollY,bottom=24+rowOffset(row+1)-scrollY;if(bottom>24&&top<height)context.fillText(String(row),20,(top+bottom)/2)}};
  content._hcdGridMetrics={sheet:content.dataset.sheet,widths,columnOffsets,targetColumns,targetRows,rowOffset,rowAtOffset,draw};renderers.push(draw);draw();
}
let frame=0;const schedule=()=>{if(frame)return;frame=requestAnimationFrame(()=>{frame=0;for(const draw of renderers)draw()})};workspace.addEventListener("scroll",schedule,{passive:true});window.addEventListener("resize",schedule,{passive:true});new MutationObserver(schedule).observe(workspace,{subtree:true,attributes:true,attributeFilter:["data-active"]});
window.hcdGridHitTest=(clientX,clientY)=>{const content=document.querySelector('.hcd-grid-sheet-content[data-active="true"]'),metrics=content?content._hcdGridMetrics:null;if(!metrics)return null;const target=document.elementFromPoint(clientX,clientY),visual=target?.closest(".hcd-sheet-picture[data-hcd-id],.hcd-sheet-chart[data-hcd-id]");if(visual&&content.contains(visual))return {sheet:metrics.sheet,cell:visual.dataset.hcdAnchorFrom||null,row:null,column:null,nodeId:visual.dataset.hcdId,nodeHash:visual.dataset.hcdNodeHash||null,nodeKind:visual.dataset.hcdNodeKind||"visual",loaded:true};const domCell=target?.closest("td[data-hcd-cell]");if(domCell&&content.contains(domCell)){const node=domCell.querySelector("[data-hcd-id]");return {sheet:metrics.sheet,cell:domCell.dataset.hcdCell,row:Number(domCell.closest("tr[data-hcd-row]")?.dataset.hcdRow)||null,column:Number(domCell.dataset.hcdColumn)||null,nodeId:node?.dataset.hcdId||null,nodeHash:node?.dataset.hcdNodeHash||null,nodeKind:node?.dataset.hcdNodeKind||"cell",loaded:Boolean(node)}}const bounds=workspace.getBoundingClientRect(),x=clientX-bounds.left+workspace.scrollLeft-40,y=clientY-bounds.top+workspace.scrollTop-24;if(x<0||y<0)return null;const column=Math.max(1,Math.min(metrics.targetColumns,upperBound(metrics.columnOffsets,x))),row=Math.max(1,Math.min(metrics.targetRows,metrics.rowAtOffset(y))),cell=columnName(column)+row;return {sheet:metrics.sheet,cell,row,column,nodeId:null,nodeHash:null,nodeKind:"cell",loaded:false}};
})();</script>"##;

/// Options shared by CLI previews, Java-side inspection downloads and future
/// profile renderers. Rendering is streaming: only one bounded HCD chunk is
/// resident at a time.
#[derive(Debug, Clone)]
pub struct HtmlPresentationOptions {
    pub revision: Option<u64>,
    pub max_output_bytes: u64,
    /// Zero-based first chunk sequence to materialize. The default starts at
    /// the beginning of the revision.
    pub chunk_start: usize,
    /// Maximum number of chunks to materialize. `None` preserves the legacy
    /// full-document behavior. This is a presentation window only and does
    /// not change the authoritative revision or its root hash.
    pub chunk_limit: Option<usize>,
    /// Prefix used to turn canonical `asset://sha256/...` references into
    /// browser-readable URLs. `None` leaves canonical references untouched.
    pub asset_base_href: Option<String>,
    /// Hover-outline state for editable text nodes. The standalone page
    /// exposes it as `body[data-hcd-text-hitboxes=on|off]` for runtime toggles.
    pub text_hitboxes_enabled: bool,
    /// Hover-outline state for image/form visual nodes. This is intentionally
    /// independent from text and exposed as
    /// `body[data-hcd-image-hitboxes=on|off]`.
    pub image_hitboxes_enabled: bool,
    /// Optional validated CSS appended after the canonical bundle stylesheet.
    /// It changes only this derived presentation and is never written into the
    /// HCD bundle or included in its root hash.
    pub override_stylesheet: Option<String>,
}

impl Default for HtmlPresentationOptions {
    fn default() -> Self {
        Self {
            revision: None,
            max_output_bytes: DEFAULT_HTML_PRESENTATION_MAX_BYTES,
            chunk_start: 0,
            chunk_limit: None,
            asset_base_href: None,
            text_hitboxes_enabled: true,
            image_hitboxes_enabled: true,
            override_stylesheet: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HtmlPresentationReport {
    pub document_id: String,
    pub revision: u64,
    pub profile: String,
    pub first_chunk: usize,
    pub chunk_count: usize,
    pub total_chunk_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_chunk: Option<usize>,
    pub bytes_written: u64,
}

/// Resolve a stable revision view without changing the bundle head.
pub fn manifest_at_revision(
    bundle: &Bundle,
    head: &HcdManifest,
    requested: Option<u64>,
) -> Result<(HcdManifest, u64), HcdError> {
    if head.revision > MAX_REVISION {
        return Err(HcdError::ResourceLimit(format!(
            "manifest revision {} exceeds the maximum {MAX_REVISION}",
            head.revision
        )));
    }
    let revision = requested.unwrap_or(head.revision);
    if revision > head.revision {
        return Err(HcdError::RevisionConflict(format!(
            "requested revision {revision} is ahead of head {}",
            head.revision
        )));
    }
    if revision == head.revision {
        return Ok((head.clone(), revision));
    }
    let record = bundle.revision(revision)?;
    let mut manifest = head.clone();
    manifest.revision = revision;
    manifest.root_hash = record.root_hash;
    manifest.annotation_root_hash = record.annotation_root_hash;
    manifest.index_prefix = record.index_prefix;
    Ok((manifest, revision))
}

/// Materialize canonical HCD fragments into one standalone inspection page.
/// This is a presentation view only; the directory bundle remains the online,
/// randomly accessible authoritative representation.
pub fn render_standalone_html(
    bundle: &Bundle,
    options: &HtmlPresentationOptions,
    output: &mut impl Write,
) -> Result<HtmlPresentationReport, HcdError> {
    render_standalone_html_with_transform(bundle, options, output, |html| Ok(html.to_string()))
}

/// Materialize a standalone page while allowing a bounded, presentation-only
/// transform for each immutable chunk. The authoritative chunk and its hash are
/// always verified before the callback runs. This keeps derived previews (for
/// example Mermaid SVG) out of canonical HCD and avoids a whole-document DOM.
pub fn render_standalone_html_with_transform(
    bundle: &Bundle,
    options: &HtmlPresentationOptions,
    output: &mut impl Write,
    mut transform: impl FnMut(&str) -> Result<String, HcdError>,
) -> Result<HtmlPresentationReport, HcdError> {
    let head = bundle.manifest()?;
    let (manifest, revision) = manifest_at_revision(bundle, &head, options.revision)?;
    let first_chunk = options.chunk_start;
    if first_chunk > manifest.chunk_count {
        return Err(HcdError::InvalidBundle(format!(
            "chunk start {first_chunk} exceeds revision {revision} chunk count {}",
            manifest.chunk_count
        )));
    }
    if options.chunk_limit == Some(0) {
        return Err(HcdError::InvalidBundle(
            "chunk limit must be greater than zero".to_string(),
        ));
    }
    let chunk_end = match options.chunk_limit {
        Some(limit) => first_chunk
            .checked_add(limit)
            .ok_or_else(|| HcdError::ResourceLimit("chunk window overflowed usize".to_string()))?
            .min(manifest.chunk_count),
        None => manifest.chunk_count,
    };
    let paginate_docx_preview =
        manifest.profile == "semantic-flow" && manifest.source.format.eq_ignore_ascii_case("docx");
    let asset_hrefs = if options.asset_base_href.is_some() {
        bundle
            .read_asset_index_for_revision(revision)?
            .into_iter()
            .map(|asset| (asset.hash, asset.href))
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    let mut written = 0u64;
    write_bounded(
        output,
        &mut written,
        options.max_output_bytes,
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>HCD revision {revision}</title><style>html{{background:#eef1f5}}body{{box-sizing:border-box;max-width:max-content;min-width:min(100%,960px);margin:24px auto;padding:24px;background:#fff;color:#111;box-shadow:0 3px 18px #0002}}body[data-hcd-profile=\"semantic-flow\"]{{box-sizing:border-box;width:min(793.7px,calc(100% - 48px));max-width:793.7px;min-width:0;padding:53.333px;font-family:HCDSans,HCDEmoji,HCDFallback,\"Noto Sans SC\",\"PingFang SC\",\"Microsoft YaHei\",Arial,sans-serif;font-size:16px;line-height:1.6}}.hcd-chunk{{content-visibility:auto;contain-intrinsic-size:auto 800px}}@media(max-width:720px){{body,body[data-hcd-profile=\"semantic-flow\"]{{width:100%;max-width:none;margin:0;padding:16px}}}}</style><style>"
        )
        .as_bytes(),
    )?;
    let styles_path = bundle.resolve_href(&manifest.styles_href)?;
    let styles_metadata = std::fs::metadata(&styles_path)?;
    if styles_metadata.len() > MAX_CONTROL_PART_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "HCD stylesheet is {} bytes; maximum is {MAX_CONTROL_PART_BYTES}",
            styles_metadata.len()
        )));
    }
    let styles = std::fs::read(&styles_path)?;
    write_bounded(output, &mut written, options.max_output_bytes, &styles)?;
    if manifest.profile == "grid" {
        write_bounded(
            output,
            &mut written,
            options.max_output_bytes,
            GRID_PRESENTATION_SHELL_CSS.as_bytes(),
        )?;
    }
    if let Some(stylesheet) = options.override_stylesheet.as_deref() {
        crate::validate_css_text(stylesheet)?;
        write_bounded(output, &mut written, options.max_output_bytes, b"\n")?;
        write_bounded(
            output,
            &mut written,
            options.max_output_bytes,
            stylesheet.as_bytes(),
        )?;
    }
    if paginate_docx_preview {
        write_bounded(
            output,
            &mut written,
            options.max_output_bytes,
            DOCX_VIRTUAL_PAGE_CSS.as_bytes(),
        )?;
    }
    write_bounded(
        output,
        &mut written,
        options.max_output_bytes,
        format!(
            "</style></head><body data-hcd-profile=\"{}\" data-hcd-source-format=\"{}\" data-hcd-revision=\"{revision}\" data-hcd-text-hitboxes=\"{}\" data-hcd-image-hitboxes=\"{}\">",
            manifest.profile,
            manifest.source.format,
            if options.text_hitboxes_enabled { "on" } else { "off" },
            if options.image_hitboxes_enabled { "on" } else { "off" }
        )
        .as_bytes(),
    )?;

    let mut expected_sequence = first_chunk;
    let first_index_page = first_chunk / crate::INDEX_PAGE_SIZE;
    let end_index_page = chunk_end.div_ceil(crate::INDEX_PAGE_SIZE);
    for page_number in first_index_page..end_index_page {
        let page = bundle.read_index_page(&manifest, page_number)?;
        if page.revision != revision || page.page != page_number {
            return Err(HcdError::InvalidBundle(format!(
                "index page {page_number} does not belong to revision {revision}"
            )));
        }
        for descriptor in page.chunks {
            if descriptor.sequence < first_chunk || descriptor.sequence >= chunk_end {
                continue;
            }
            if descriptor.sequence != expected_sequence {
                return Err(HcdError::InvalidBundle(format!(
                    "expected chunk sequence {expected_sequence}, found {}",
                    descriptor.sequence
                )));
            }
            let html = bundle.read_chunk(&descriptor)?;
            if html.len() > MAX_CHUNK_BYTES || html.len() as u64 != descriptor.byte_length {
                return Err(HcdError::InvalidBundle(format!(
                    "chunk {} byte length mismatch",
                    descriptor.chunk_id
                )));
            }
            let actual_hash = hash_bytes(html.as_bytes());
            if actual_hash != descriptor.html_hash {
                return Err(HcdError::InvalidBundle(format!(
                    "chunk {} expected hash {}, actual {actual_hash}",
                    descriptor.chunk_id, descriptor.html_hash
                )));
            }
            let presented_html = if let Some(base_href) = &options.asset_base_href {
                rewrite_asset_references(&html, &asset_hrefs, base_href)
            } else {
                html
            };
            let presented_html = transform(&presented_html)?;
            if presented_html.len() > MAX_CHUNK_BYTES.saturating_mul(4) {
                return Err(HcdError::ResourceLimit(format!(
                    "presentation transform expanded chunk {} beyond {} bytes",
                    descriptor.chunk_id,
                    MAX_CHUNK_BYTES.saturating_mul(4)
                )));
            }
            write_bounded(
                output,
                &mut written,
                options.max_output_bytes,
                presented_html.as_bytes(),
            )?;
            expected_sequence += 1;
        }
    }
    if expected_sequence != chunk_end {
        return Err(HcdError::InvalidBundle(format!(
            "requested chunk window {first_chunk}..{chunk_end}, materialized through {expected_sequence}"
        )));
    }
    if manifest.profile == "grid" {
        write_bounded(
            output,
            &mut written,
            options.max_output_bytes,
            GRID_PRESENTATION_SCRIPT.as_bytes(),
        )?;
        write_bounded(
            output,
            &mut written,
            options.max_output_bytes,
            GRID_GEOMETRY_SCRIPT.as_bytes(),
        )?;
    }
    if paginate_docx_preview {
        write_bounded(
            output,
            &mut written,
            options.max_output_bytes,
            DOCX_VIRTUAL_PAGE_SCRIPT.as_bytes(),
        )?;
    }
    write_bounded(
        output,
        &mut written,
        options.max_output_bytes,
        b"</body></html>",
    )?;
    Ok(HtmlPresentationReport {
        document_id: manifest.document_id,
        revision,
        profile: manifest.profile,
        first_chunk,
        chunk_count: chunk_end - first_chunk,
        total_chunk_count: manifest.chunk_count,
        next_chunk: (chunk_end < manifest.chunk_count).then_some(chunk_end),
        bytes_written: written,
    })
}

const DOCX_VIRTUAL_PAGE_CSS: &str = r#"
body.hcd-docx-paginated{width:auto!important;max-width:none!important;min-width:0!important;min-height:0!important;margin:24px auto!important;padding:0!important;background:transparent!important;box-shadow:none!important;display:flex!important;flex-direction:column;align-items:center;gap:20px}
body.hcd-docx-paginated>.hcd-virtual-page{box-sizing:border-box;flex:0 0 auto;position:relative;overflow:hidden;background:#fff;box-shadow:0 3px 18px #0002;content-visibility:visible;contain:none}
body.hcd-docx-paginated>.hcd-virtual-page[data-hcd-page-overflow="true"]{height:auto!important;overflow:visible}
body.hcd-docx-paginated .hcd-virtual-page-content{box-sizing:border-box;width:100%;overflow:visible;content-visibility:visible;contain:none}
@media(max-width:720px){body.hcd-docx-paginated{margin:0!important;gap:12px}body.hcd-docx-paginated>.hcd-virtual-page{max-width:100%}}
"#;

const DOCX_VIRTUAL_PAGE_SCRIPT: &str = r#"<script id="hcd-docx-virtual-pages">
(()=>{const paginate=()=>{const body=document.body;if(body.classList.contains("hcd-docx-paginated"))return;const chunks=[...body.querySelectorAll(":scope > section.hcd-chunk")];if(!chunks.length)return;const cs=getComputedStyle(body),rect=body.getBoundingClientRect();const number=name=>Number.parseFloat(cs[name])||0;const width=rect.width,height=number("minHeight")||rect.height,top=number("paddingTop"),right=number("paddingRight"),bottom=number("paddingBottom"),left=number("paddingLeft"),available=Math.max(1,height-top-bottom);const blocks=[];for(const chunk of chunks){const chunkId=chunk.dataset.hcdChunkId||"";for(const node of [...chunk.childNodes]){if(node.nodeType===1)blocks.push({node,chunkId});else if(node.nodeType===3&&node.textContent.trim())blocks.push({node,chunkId});}}body.replaceChildren();body.classList.add("hcd-docx-paginated");let pageNo=0,page,content,count=0;const newPage=()=>{page=document.createElement("section");page.className="hcd-virtual-page hcd-chunk";page.dataset.hcdVirtualPage=String(++pageNo);page.style.width=`${width}px`;page.style.height=`${height}px`;page.style.padding=`${top}px ${right}px ${bottom}px ${left}px`;content=document.createElement("div");content.className="hcd-virtual-page-content";content.style.height=`${available}px`;page.append(content);body.append(page);count=0;};newPage();for(const block of blocks){content.append(block.node);count++;if(block.chunkId){const ids=new Set((page.dataset.hcdChunkIds||"").split(",").filter(Boolean));ids.add(block.chunkId);page.dataset.hcdChunkIds=[...ids].join(",");}if(content.scrollHeight>available+.5){if(count>1){content.removeChild(block.node);newPage();content.append(block.node);count=1;if(block.chunkId)page.dataset.hcdChunkIds=block.chunkId;}if(content.scrollHeight>available+.5&&count===1){page.dataset.hcdPageOverflow="true";page.style.height="auto";content.style.height="auto";}}}body.dataset.hcdPageCount=String(pageNo);};const ready=()=>Promise.all([document.fonts?.ready||Promise.resolve(),...([...document.images].map(img=>img.complete?Promise.resolve():new Promise(resolve=>{img.addEventListener("load",resolve,{once:true});img.addEventListener("error",resolve,{once:true});}))) ]).then(paginate);if(document.readyState==="complete")ready();else window.addEventListener("load",ready,{once:true});})();
</script>"#;

fn rewrite_asset_references(
    html: &str,
    asset_hrefs: &HashMap<String, String>,
    base_href: &str,
) -> String {
    const PREFIX: &str = "asset://sha256/";
    let mut output = String::with_capacity(html.len());
    let mut remainder = html;
    while let Some(offset) = remainder.find(PREFIX) {
        output.push_str(&remainder[..offset]);
        let candidate = &remainder[offset + PREFIX.len()..];
        let hash_length = candidate
            .bytes()
            .take_while(|byte| byte.is_ascii_hexdigit())
            .count();
        let hash = &candidate[..hash_length];
        if let Some(href) = asset_hrefs.get(hash) {
            output.push_str(base_href);
            output.push_str(href);
            remainder = &candidate[hash_length..];
        } else {
            output.push_str(PREFIX);
            remainder = candidate;
        }
    }
    output.push_str(remainder);
    output
}

fn write_bounded(
    output: &mut impl Write,
    written: &mut u64,
    maximum: u64,
    bytes: &[u8],
) -> Result<(), HcdError> {
    let next = written
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| HcdError::ResourceLimit("HTML byte count overflowed".to_string()))?;
    if next > maximum {
        return Err(HcdError::ResourceLimit(format!(
            "standalone HCD HTML exceeds the {maximum} byte output limit"
        )));
    }
    output.write_all(bytes)?;
    *written = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_checks_the_limit_before_emitting_more_bytes() {
        let mut output = Vec::new();
        let mut written = 1;
        let error = write_bounded(&mut output, &mut written, 1, b"x").unwrap_err();
        assert!(error.to_string().contains("output limit"));
        assert!(output.is_empty());
        assert_eq!(written, 1);
    }

    #[test]
    fn presentation_rewrites_known_asset_uris_only() {
        let hash = "ab".repeat(32);
        let hrefs = HashMap::from([(hash.clone(), format!("assets/sha256/{hash}.png"))]);
        let html =
            format!("<img src=\"asset://sha256/{hash}\"><img src=\"asset://sha256/unknown\">");
        let rewritten = rewrite_asset_references(&html, &hrefs, "../bundle/");
        assert!(rewritten.contains(&format!("src=\"../bundle/assets/sha256/{hash}.png\"")));
        assert!(rewritten.contains("src=\"asset://sha256/unknown\""));
    }

    #[test]
    fn semantic_flow_preview_metrics_match_a4_with_forty_point_margins() {
        const A4_WIDTH_CSS_PX: f64 = 793.7;
        const FORTY_POINTS_CSS_PX: f64 = 53.333;
        const A4_CONTENT_WIDTH_CSS_PX: f64 = 687.034;
        assert!(
            (A4_WIDTH_CSS_PX - (FORTY_POINTS_CSS_PX * 2.0) - A4_CONTENT_WIDTH_CSS_PX).abs() < 0.001
        );
    }

    #[test]
    fn docx_virtual_pagination_preserves_nodes_and_uses_measured_page_geometry() {
        assert!(DOCX_VIRTUAL_PAGE_SCRIPT.contains("for(const node of [...chunk.childNodes])"));
        assert!(DOCX_VIRTUAL_PAGE_SCRIPT.contains("content.append(block.node)"));
        assert!(DOCX_VIRTUAL_PAGE_SCRIPT.contains("content.scrollHeight>available+.5"));
        assert!(DOCX_VIRTUAL_PAGE_SCRIPT.contains("body.dataset.hcdPageCount"));
        assert!(!DOCX_VIRTUAL_PAGE_SCRIPT.contains("cloneNode"));
        assert!(DOCX_VIRTUAL_PAGE_CSS.contains(".hcd-virtual-page"));
        assert!(DOCX_VIRTUAL_PAGE_CSS.contains("content-visibility:visible"));
    }

    #[test]
    fn grid_presentation_groups_sheet_chunks_and_builds_excel_chrome() {
        assert!(GRID_PRESENTATION_SHELL_CSS.contains(".hcd-grid-title"));
        assert!(GRID_PRESENTATION_SHELL_CSS.contains(".hcd-grid-tabs"));
        assert!(GRID_PRESENTATION_SHELL_CSS.contains(".hcd-grid-sheet-content"));
        assert!(GRID_PRESENTATION_SHELL_CSS.contains(".hcd-column-header"));
        assert!(GRID_PRESENTATION_SHELL_CSS.contains(".hcd-row-header"));
        assert!(GRID_PRESENTATION_SHELL_CSS.contains(".hcd-grid-canvas"));
        assert!(GRID_PRESENTATION_SCRIPT.contains("querySelectorAll(\"[data-hcd-sheet]\")"));
        assert!(GRID_PRESENTATION_SCRIPT.contains("sheet.regions.push(region)"));
        assert!(GRID_PRESENTATION_SCRIPT.contains("sheet.state===\"visible\""));
        assert!(GRID_PRESENTATION_SCRIPT.contains("role\",\"tablist"));
        assert!(GRID_PRESENTATION_SCRIPT.contains("content.append(region)"));
        assert!(GRID_PRESENTATION_SCRIPT.contains("sheet.content.dataset.active"));
        assert!(GRID_PRESENTATION_SCRIPT.contains("col.style.width=\"64px\""));
        assert!(!GRID_PRESENTATION_SCRIPT.contains("hcd-grid-formula-bar"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("data-hcd-anchor-from"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("DEFAULT_COLUMN_EMU=609600"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("DEFAULT_ROW_EMU=190500"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("MAX_VISIBLE_LINES=2048"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("requestAnimationFrame"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("window.hcdGridHitTest"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("nodeId:node?.dataset.hcdId"));
        assert!(!GRID_GEOMETRY_SCRIPT.contains("const nodeIndex=new Map"));
        assert!(!GRID_GEOMETRY_SCRIPT.contains("document.createElement(\"td\")"));
        assert!(GRID_GEOMETRY_SCRIPT.contains("visual.style.left"));
    }

    #[test]
    fn preview_style_override_is_safe_and_opt_in() {
        let default = HtmlPresentationOptions::default();
        assert!(default.override_stylesheet.is_none());
        assert_eq!(default.chunk_start, 0);
        assert_eq!(default.chunk_limit, None);
        assert!(crate::validate_css_text(".hcd-grid td{color:#123456}").is_ok());
        assert!(crate::validate_css_text(".hcd-grid{background:url(javascript:x)}").is_err());
    }
}
