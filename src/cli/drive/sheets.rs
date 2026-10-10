//! CLI commands for `gwi drive sheets` — reading and writing the
//! *cells* of a Google Sheet via the Sheets v4 API (issue #1589), editing
//! its *structure* via `spreadsheets.batchUpdate` (issue #1613),
//! *destructively* editing it the same way (issue #1623), and applying
//! formatting, data validation and protected ranges (issue #1643), the
//! basic filter and filter views (issue #1794), conditional formatting
//! rules (issue #1793), and charts and slicers (issue #1797).
//!
//! Nested under `drive` rather than given its own top-level tree so it
//! inherits `--account` resolution, the `auth` commands and the write-
//! permission diagnostics: a Sheet is a Drive file, and the permission gate
//! is a Drive concept.

pub(crate) mod auto_fill;
pub(crate) mod banding;
pub(crate) mod cell_format;
pub(crate) mod conditional_format;
pub(crate) mod create;
pub(crate) mod delete_duplicates;
pub(crate) mod developer_metadata;
pub(crate) mod dimension_group;
pub(crate) mod embedded_object;
pub(crate) mod filter;
pub(crate) mod find_replace;
pub(crate) mod format;
pub(crate) mod info;
pub(crate) mod named_range;
pub(crate) mod paste;
pub(crate) mod pivot;
pub(crate) mod protection;
pub(crate) mod randomize_range;
pub(crate) mod read;
pub(crate) mod sort_range;
pub(crate) mod structure;
pub(crate) mod text_to_columns;
pub(crate) mod trim_whitespace;
pub(crate) mod validation;
pub(crate) mod values;
pub(crate) mod write;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::drive::client::DriveClient;

/// Reads and writes the cells and structure of a Google Sheet.
#[derive(Parser)]
pub struct SheetsCommand {
    /// The sheets subcommand to execute.
    #[command(subcommand)]
    pub command: SheetsSubcommands,
}

/// Sheets subcommands.
#[derive(Subcommand)]
pub enum SheetsSubcommands {
    /// Shows a spreadsheet's title and the sheets (tabs) it contains.
    Info(info::InfoCommand),
    /// Reads cell values from one range, or from every sheet.
    Read(read::ReadCommand),
    /// Reads a range's cell-level formatting back — background, text format, number
    /// format, horizontal alignment, notes and data validation. Read-only and ungated,
    /// like `sheets read` (issue rust-works/omni-dev#1878): it discloses no more than
    /// opening the file in the UI does. The tool ADR-0083 §5 relies on to answer
    /// whether a verb moves formatting.
    #[command(about = "Reads a range's cell-level formatting back — background, text format, number format, horizontal alignment, notes and data validation. Read-only and ungated, like sheets read (issue rust-works/omni-dev#1878): it discloses no more than opening the file in the UI does. The tool ADR-0083 §5 relies on to answer whether a verb moves formatting", long_about = None)]
    ReadCellFormat(cell_format::ReadCellFormatCommand),
    /// Overwrites the cells of a range, gated by the write-permission rules (issues
    /// rust-works/omni-dev#1589, rust-works/omni-dev#1612). Requires the `drive.file`
    /// or `drive` scope (`drive auth login --write-file`/`--write-full`). Drops a
    /// cell's rich-text runs even when the value is unchanged; see `docs/drive.md`.
    /// (mirrors the `drive_sheets_write` MCP tool).
    #[command(about = "Overwrites the cells of a range, gated by the write-permission rules (issues rust-works/omni-dev#1589, rust-works/omni-dev#1612). Requires the drive.file or drive scope (drive auth login --write-file/--write-full). Drops a cell's rich-text runs even when the value is unchanged; see docs/drive.md. (mirrors the drive_sheets_write MCP tool)", long_about = None)]
    Write(write::WriteCommand),
    /// Appends rows after the last row of a range's table, gated by the
    /// write-permission rules (issues rust-works/omni-dev#1589,
    /// rust-works/omni-dev#1612). Presumably drops rich-text runs on an existing cell
    /// like `sheets write` does, though this hasn't been measured; see `docs/drive.md`.
    /// (mirrors the `drive_sheets_append` MCP tool).
    #[command(about = "Appends rows after the last row of a range's table, gated by the write-permission rules (issues rust-works/omni-dev#1589, rust-works/omni-dev#1612). Presumably drops rich-text runs on an existing cell like sheets write does, though this hasn't been measured; see docs/drive.md. (mirrors the drive_sheets_append MCP tool)", long_about = None)]
    Append(write::AppendCommand),
    /// Clears a range's values, leaving formatting intact. Gated by the
    /// write-permission rules (issues rust-works/omni-dev#1589,
    /// rust-works/omni-dev#1612). (mirrors the `drive_sheets_clear` MCP tool).
    #[command(about = "Clears a range's values, leaving formatting intact. Gated by the write-permission rules (issues rust-works/omni-dev#1589, rust-works/omni-dev#1612). (mirrors the drive_sheets_clear MCP tool)", long_about = None)]
    Clear(write::ClearCommand),
    /// Finds text and replaces it across a range, one sheet, or the workbook. Gated by
    /// `sheets-write` (issue rust-works/omni-dev#1841, ADR-0083 §1).
    #[command(about = "Finds text and replaces it across a range, one sheet, or the workbook. Gated by sheets-write (issue rust-works/omni-dev#1841, ADR-0083 §1)", long_about = None)]
    FindReplace(find_replace::FindReplaceCommand),
    /// Creates a new Google Sheet, optionally seeded with values. Gated by the folder
    /// write-permission rules' `create` operation (issue rust-works/omni-dev#1589).
    #[command(about = "Creates a new Google Sheet, optionally seeded with values. Gated by the folder write-permission rules' create operation (issue rust-works/omni-dev#1589)", long_about = None)]
    Create(create::CreateCommand),
    /// Adds a new sheet (tab) to a spreadsheet. Gated by the folder write-permission
    /// rules' `sheets-structure` operation (issue rust-works/omni-dev#1613).
    #[command(about = "Adds a new sheet (tab) to a spreadsheet. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1613)", long_about = None)]
    AddSheet(structure::AddSheetCommand),
    /// Renames an existing sheet. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1613).
    #[command(about = "Renames an existing sheet. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1613)", long_about = None)]
    RenameSheet(structure::RenameSheetCommand),
    /// Inserts empty rows, shifting existing rows down. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1613).
    #[command(about = "Inserts empty rows, shifting existing rows down. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1613)", long_about = None)]
    InsertRows(structure::InsertRowsCommand),
    /// Inserts empty columns, shifting existing columns right. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1613).
    #[command(about = "Inserts empty columns, shifting existing columns right. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1613)", long_about = None)]
    InsertColumns(structure::InsertColumnsCommand),
    /// Inserts empty cells into a rectangular range, shifting existing cells down or
    /// right. Gated by the folder write-permission rules' `sheets-structure` operation
    /// (issue rust-works/omni-dev#1838).
    #[command(about = "Inserts empty cells into a rectangular range, shifting existing cells down or right. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1838)", long_about = None)]
    InsertRange(structure::InsertRangeCommand),
    /// Moves a contiguous block of rows to a new position within a sheet, shifting the
    /// rows in between. Gated by the folder write-permission rules' `sheets-structure`
    /// operation (issue rust-works/omni-dev#1834).
    #[command(about = "Moves a contiguous block of rows to a new position within a sheet, shifting the rows in between. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1834)", long_about = None)]
    MoveRows(structure::MoveRowsCommand),
    /// Moves a contiguous block of columns to a new position within a sheet, shifting
    /// the columns in between. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1834).
    #[command(about = "Moves a contiguous block of columns to a new position within a sheet, shifting the columns in between. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1834)", long_about = None)]
    MoveColumns(structure::MoveColumnsCommand),
    /// Deletes an entire sheet (tab) from a spreadsheet. Gated by the folder
    /// write-permission rules' `sheets-delete` operation (issue
    /// rust-works/omni-dev#1623). Cannot be undone through gwi.
    #[command(about = "Deletes an entire sheet (tab) from a spreadsheet. Gated by the folder write-permission rules' sheets-delete operation (issue rust-works/omni-dev#1623). Cannot be undone through gwi", long_about = None)]
    DeleteSheet(structure::DeleteSheetCommand),
    /// Deletes whole rows, shifting existing rows up. Gated by the folder
    /// write-permission rules' `sheets-delete` operation (issue
    /// rust-works/omni-dev#1623). Cannot be undone through gwi.
    #[command(about = "Deletes whole rows, shifting existing rows up. Gated by the folder write-permission rules' sheets-delete operation (issue rust-works/omni-dev#1623). Cannot be undone through gwi", long_about = None)]
    DeleteRows(structure::DeleteRowsCommand),
    /// Deletes whole columns, shifting existing columns left. Gated by the folder
    /// write-permission rules' `sheets-delete` operation (issue
    /// rust-works/omni-dev#1623). Cannot be undone through gwi.
    #[command(about = "Deletes whole columns, shifting existing columns left. Gated by the folder write-permission rules' sheets-delete operation (issue rust-works/omni-dev#1623). Cannot be undone through gwi", long_about = None)]
    DeleteColumns(structure::DeleteColumnsCommand),
    /// Deletes a rectangular cell range, shifting the remainder along one axis. Gated
    /// by the folder write-permission rules' `sheets-delete` operation (issue
    /// rust-works/omni-dev#1623). Cannot be undone through gwi.
    #[command(about = "Deletes a rectangular cell range, shifting the remainder along one axis. Gated by the folder write-permission rules' sheets-delete operation (issue rust-works/omni-dev#1623). Cannot be undone through gwi", long_about = None)]
    DeleteRange(structure::DeleteRangeCommand),
    /// Copies an existing sheet within the same workbook. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Copies an existing sheet within the same workbook. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    DuplicateSheet(structure::DuplicateSheetCommand),
    /// Moves an existing sheet to a new position among its siblings. Gated by the
    /// folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Moves an existing sheet to a new position among its siblings. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    ReorderSheet(structure::ReorderSheetCommand),
    /// Hides an existing sheet. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1643).
    #[command(about = "Hides an existing sheet. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    HideSheet(structure::HideSheetCommand),
    /// Shows an existing hidden sheet. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1643).
    #[command(about = "Shows an existing hidden sheet. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    ShowSheet(structure::ShowSheetCommand),
    /// Changes a sheet's view properties — frozen rows/columns, tab color,
    /// right-to-left, hidden gridlines. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1835).
    #[command(about = "Changes a sheet's view properties — frozen rows/columns, tab color, right-to-left, hidden gridlines. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1835)", long_about = None)]
    UpdateSheetProperties(structure::UpdateSheetPropertiesCommand),
    /// Changes workbook-level properties: locale, time zone, automatic recalculation,
    /// and iterative calculation. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1836).
    #[command(about = "Changes workbook-level properties: locale, time zone, automatic recalculation, and iterative calculation. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1836)", long_about = None)]
    UpdateWorkbookProperties(structure::UpdateWorkbookPropertiesCommand),
    /// Applies a cell format across a range. Gated by the folder write-permission
    /// rules' `sheets-structure` operation (issue rust-works/omni-dev#1643).
    #[command(about = "Applies a cell format across a range. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    FormatCells(format::FormatCellsCommand),
    /// Sets border lines on a range's edges. Gated by the folder write-permission
    /// rules' `sheets-structure` operation (issue rust-works/omni-dev#1643).
    #[command(about = "Sets border lines on a range's edges. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    UpdateBorders(format::UpdateBordersCommand),
    /// Merges a range into one cell, discarding every value but the top-left's. Gated
    /// by the folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Merges a range into one cell, discarding every value but the top-left's. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    MergeCells(format::MergeCellsCommand),
    /// Splits a previously merged range back apart. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Splits a previously merged range back apart. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    UnmergeCells(format::UnmergeCellsCommand),
    /// Resizes rows or columns to fit their content. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Resizes rows or columns to fit their content. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    AutoResizeDimension(format::AutoResizeDimensionCommand),
    /// Sets an explicit pixel width (columns) or height (rows). Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Sets an explicit pixel width (columns) or height (rows). Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    UpdateDimensionProperties(format::UpdateDimensionPropertiesCommand),
    /// Sets a data validation rule on a range. Gated by the folder write-permission
    /// rules' `sheets-structure` operation (issue rust-works/omni-dev#1643).
    ///
    /// Boxed (rust-works/omni-dev#1792): tranche 2's ~19 extra condition flags pushed
    /// this variant far past every sibling's size, which `clippy::large_enum_variant`
    /// flags transitively up through `SheetsSubcommands`/`DriveSubcommands`/
    /// `Commands`.
    #[command(
        about = "Sets a data validation rule on a range. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)",
        long_about = "Sets a data validation rule on a range. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643).\n\nBoxed (rust-works/omni-dev#1792): tranche 2's ~19 extra condition flags pushed this variant far past every sibling's size, which clippy::large_enum_variant flags transitively up through SheetsSubcommands/DriveSubcommands/ Commands."
    )]
    SetDataValidation(Box<validation::SetDataValidationCommand>),
    /// Removes a range's data validation rule. Gated by the folder write-permission
    /// rules' `sheets-structure` operation (issue rust-works/omni-dev#1643).
    #[command(about = "Removes a range's data validation rule. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1643)", long_about = None)]
    ClearDataValidation(validation::ClearDataValidationCommand),
    /// Creates or updates a developer-metadata key/value pair on a spreadsheet, sheet,
    /// row or column. Restricted to `DOCUMENT` visibility. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1795).
    #[command(about = "Creates or updates a developer-metadata key/value pair on a spreadsheet, sheet, row or column. Restricted to DOCUMENT visibility. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1795)", long_about = None)]
    SetDeveloperMetadata(developer_metadata::SetDeveloperMetadataCommand),
    /// Removes developer metadata matching a key and location, after reporting what
    /// would be removed. Restricted to `DOCUMENT` visibility. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1795).
    #[command(about = "Removes developer metadata matching a key and location, after reporting what would be removed. Restricted to DOCUMENT visibility. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1795)", long_about = None)]
    DeleteDeveloperMetadata(developer_metadata::DeleteDeveloperMetadataCommand),
    /// Searches for developer metadata by key and/or location, restricted to `DOCUMENT`
    /// visibility. Read-only and ungated, like `sheets info` (issue
    /// rust-works/omni-dev#1795).
    #[command(about = "Searches for developer metadata by key and/or location, restricted to DOCUMENT visibility. Read-only and ungated, like sheets info (issue rust-works/omni-dev#1795)", long_about = None)]
    SearchDeveloperMetadata(developer_metadata::SearchDeveloperMetadataCommand),
    /// Protects a range or an entire sheet. Gated by the folder write-permission rules'
    /// `sheets-protection` operation — distinct from `sheets-structure` (issue
    /// rust-works/omni-dev#1643).
    #[command(about = "Protects a range or an entire sheet. Gated by the folder write-permission rules' sheets-protection operation — distinct from sheets-structure (issue rust-works/omni-dev#1643)", long_about = None)]
    ProtectRange(protection::ProtectRangeCommand),
    /// Changes an existing protected range's description, warning-only flag, or editor
    /// list. Gated by the folder write-permission rules' `sheets-protection` operation
    /// (issue rust-works/omni-dev#1643).
    #[command(about = "Changes an existing protected range's description, warning-only flag, or editor list. Gated by the folder write-permission rules' sheets-protection operation (issue rust-works/omni-dev#1643)", long_about = None)]
    UpdateProtection(protection::UpdateProtectionCommand),
    /// Removes a protected range. Gated by the folder write-permission rules'
    /// `sheets-protection` operation (issue rust-works/omni-dev#1643).
    #[command(about = "Removes a protected range. Gated by the folder write-permission rules' sheets-protection operation (issue rust-works/omni-dev#1643)", long_about = None)]
    UnprotectRange(protection::UnprotectRangeCommand),
    /// Lists the protected ranges in a spreadsheet. Read-only and ungated, like `sheets
    /// info` (issue rust-works/omni-dev#1643).
    #[command(about = "Lists the protected ranges in a spreadsheet. Read-only and ungated, like sheets info (issue rust-works/omni-dev#1643)", long_about = None)]
    ListProtections(protection::ListProtectionsCommand),
    /// Sets (upserting any existing one) the basic filter on a sheet. Gated by the
    /// folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1794).
    #[command(about = "Sets (upserting any existing one) the basic filter on a sheet. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1794)", long_about = None)]
    SetBasicFilter(filter::SetBasicFilterCommand),
    /// Removes a sheet's basic filter. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1794).
    #[command(about = "Removes a sheet's basic filter. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1794)", long_about = None)]
    ClearBasicFilter(filter::ClearBasicFilterCommand),
    /// Adds a named filter view. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1794).
    #[command(about = "Adds a named filter view. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1794)", long_about = None)]
    AddFilterView(filter::AddFilterViewCommand),
    /// Changes an existing filter view's title, range, sort order, or hidden values.
    /// Gated by the folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1794).
    #[command(about = "Changes an existing filter view's title, range, sort order, or hidden values. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1794)", long_about = None)]
    UpdateFilterView(filter::UpdateFilterViewCommand),
    /// Removes a filter view. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1794).
    #[command(about = "Removes a filter view. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1794)", long_about = None)]
    DeleteFilterView(filter::DeleteFilterViewCommand),
    /// Lists the filter views in a spreadsheet. Read-only and ungated, like
    /// `list-protections` (issue rust-works/omni-dev#1794).
    #[command(about = "Lists the filter views in a spreadsheet. Read-only and ungated, like list-protections (issue rust-works/omni-dev#1794)", long_about = None)]
    ListFilterViews(filter::ListFilterViewsCommand),
    /// Adds a conditional format rule to one or more ranges. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1793, ADR-0081 §1).
    ///
    /// Boxed for the same `clippy::large_enum_variant` reason as `SetDataValidation`
    /// (rust-works/omni-dev#1792): the condition/gradient flag set is wide.
    #[command(
        about = "Adds a conditional format rule to one or more ranges. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1793, ADR-0081 §1)",
        long_about = "Adds a conditional format rule to one or more ranges. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1793, ADR-0081 §1).\n\nBoxed for the same clippy::large_enum_variant reason as SetDataValidation (rust-works/omni-dev#1792): the condition/gradient flag set is wide."
    )]
    AddConditionalFormat(Box<conditional_format::AddConditionalFormatCommand>),
    /// Replaces the conditional format rule at an index. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1793, ADR-0081 §1).
    #[command(about = "Replaces the conditional format rule at an index. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1793, ADR-0081 §1)", long_about = None)]
    UpdateConditionalFormat(Box<conditional_format::UpdateConditionalFormatCommand>),
    /// Removes the conditional format rule at an index. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1793, ADR-0081 §1).
    #[command(about = "Removes the conditional format rule at an index. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1793, ADR-0081 §1)", long_about = None)]
    DeleteConditionalFormat(conditional_format::DeleteConditionalFormatCommand),
    /// Lists the conditional format rules in a spreadsheet. Read-only and ungated, like
    /// `list-protections` (issue rust-works/omni-dev#1793).
    #[command(about = "Lists the conditional format rules in a spreadsheet. Read-only and ungated, like list-protections (issue rust-works/omni-dev#1793)", long_about = None)]
    ListConditionalFormats(conditional_format::ListConditionalFormatsCommand),
    /// Adds a named range. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1796).
    #[command(about = "Adds a named range. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1796)", long_about = None)]
    AddNamedRange(named_range::AddNamedRangeCommand),
    /// Changes an existing named range's name and/or the range it covers. Gated by the
    /// folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1796).
    #[command(about = "Changes an existing named range's name and/or the range it covers. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1796)", long_about = None)]
    UpdateNamedRange(named_range::UpdateNamedRangeCommand),
    /// Removes a named range. Gated by the folder write-permission rules'
    /// `sheets-structure` operation — not `sheets-delete`, since a named range is a
    /// label, not grid data (issue rust-works/omni-dev#1796, ADR-0081 §2). Reports
    /// every cell formula that referenced the name before removing it.
    #[command(about = "Removes a named range. Gated by the folder write-permission rules' sheets-structure operation — not sheets-delete, since a named range is a label, not grid data (issue rust-works/omni-dev#1796, ADR-0081 §2). Reports every cell formula that referenced the name before removing it", long_about = None)]
    DeleteNamedRange(named_range::DeleteNamedRangeCommand),
    /// Lists the named ranges in a spreadsheet. Read-only and ungated, like `sheets
    /// list-protections` (issue rust-works/omni-dev#1796).
    #[command(about = "Lists the named ranges in a spreadsheet. Read-only and ungated, like sheets list-protections (issue rust-works/omni-dev#1796)", long_about = None)]
    ListNamedRanges(named_range::ListNamedRangesCommand),
    /// Adds a chart. Gated by the folder write-permission rules' `sheets-structure`
    /// operation (issue rust-works/omni-dev#1797, ADR-0081 §3).
    ///
    /// Boxed for the same `clippy::large_enum_variant` reason as `SetDataValidation`
    /// (rust-works/omni-dev#1792): the chart-spec flag set is wide.
    #[command(
        about = "Adds a chart. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3)",
        long_about = "Adds a chart. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3).\n\nBoxed for the same clippy::large_enum_variant reason as SetDataValidation (rust-works/omni-dev#1792): the chart-spec flag set is wide."
    )]
    AddChart(Box<embedded_object::AddChartCommand>),
    /// Replaces an existing chart's spec wholesale — `updateChartSpec` carries no field
    /// mask. Gated by the folder write-permission rules' `sheets-structure` operation
    /// (issue rust-works/omni-dev#1797, ADR-0081 §3).
    #[command(about = "Replaces an existing chart's spec wholesale — updateChartSpec carries no field mask. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3)", long_about = None)]
    UpdateChart(Box<embedded_object::UpdateChartCommand>),
    /// Removes a chart, after reporting its spec (type, title, anchor). Gated by the
    /// folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1797, ADR-0081 §3) — not `sheets-delete`; see that ADR
    /// section for why an unrecoverable embedded-object removal still sits here. Cannot
    /// be undone through gwi.
    #[command(about = "Removes a chart, after reporting its spec (type, title, anchor). Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3) — not sheets-delete; see that ADR section for why an unrecoverable embedded-object removal still sits here. Cannot be undone through gwi", long_about = None)]
    DeleteChart(embedded_object::DeleteChartCommand),
    /// Lists the charts in a spreadsheet. Read-only and ungated, like
    /// `list-protections` (issue rust-works/omni-dev#1797).
    #[command(about = "Lists the charts in a spreadsheet. Read-only and ungated, like list-protections (issue rust-works/omni-dev#1797)", long_about = None)]
    ListCharts(embedded_object::ListChartsCommand),
    /// Adds a slicer. Gated by the folder write-permission rules' `sheets-structure`
    /// operation (issue rust-works/omni-dev#1797, ADR-0081 §3).
    #[command(about = "Adds a slicer. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3)", long_about = None)]
    AddSlicer(embedded_object::AddSlicerCommand),
    /// Changes an existing slicer's range, filter column/criteria, title, or
    /// pivot-table linkage. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1797, ADR-0081 §3).
    #[command(about = "Changes an existing slicer's range, filter column/criteria, title, or pivot-table linkage. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3)", long_about = None)]
    UpdateSlicer(embedded_object::UpdateSlicerCommand),
    /// Removes a slicer, after reporting its spec. Gated by the folder write-permission
    /// rules' `sheets-structure` operation (issue rust-works/omni-dev#1797, ADR-0081
    /// §3) — not `sheets-delete`. Cannot be undone through gwi.
    #[command(about = "Removes a slicer, after reporting its spec. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1797, ADR-0081 §3) — not sheets-delete. Cannot be undone through gwi", long_about = None)]
    DeleteSlicer(embedded_object::DeleteSlicerCommand),
    /// Lists the slicers in a spreadsheet. Read-only and ungated, like
    /// `list-protections` (issue rust-works/omni-dev#1797).
    #[command(about = "Lists the slicers in a spreadsheet. Read-only and ungated, like list-protections (issue rust-works/omni-dev#1797)", long_about = None)]
    ListSlicers(embedded_object::ListSlicersCommand),
    /// Moves and/or resizes an existing chart (`updateEmbeddedObjectPosition`). Gated
    /// by the folder write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1837, ADR-0081 §3) — the same operation as `add-chart`'s own
    /// placement, since a move discards no data.
    #[command(about = "Moves and/or resizes an existing chart (updateEmbeddedObjectPosition). Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1837, ADR-0081 §3) — the same operation as add-chart's own placement, since a move discards no data", long_about = None)]
    MoveChart(embedded_object::MoveChartCommand),
    /// Moves and/or resizes an existing slicer. Same gate as `move-chart` (issue
    /// rust-works/omni-dev#1837, ADR-0081 §3).
    #[command(about = "Moves and/or resizes an existing slicer. Same gate as move-chart (issue rust-works/omni-dev#1837, ADR-0081 §3)", long_about = None)]
    MoveSlicer(embedded_object::MoveSlicerCommand),
    /// Sets or clears an existing chart's border colour (`updateEmbeddedObjectBorder`).
    /// Same gate as `move-chart` (issue rust-works/omni-dev#1837, ADR-0081 §3). Charts
    /// only — a slicer has no border.
    #[command(about = "Sets or clears an existing chart's border colour (updateEmbeddedObjectBorder). Same gate as move-chart (issue rust-works/omni-dev#1837, ADR-0081 §3). Charts only — a slicer has no border", long_about = None)]
    UpdateChartBorder(embedded_object::UpdateChartBorderCommand),
    /// Writes a new pivot table at an anchor cell. Gated by **both** the folder
    /// write-permission rules' `sheets-write` and `sheets-structure` operations (issue
    /// rust-works/omni-dev#1798, ADR-0081 §5).
    #[command(about = "Writes a new pivot table at an anchor cell. Gated by both the folder write-permission rules' sheets-write and sheets-structure operations (issue rust-works/omni-dev#1798, ADR-0081 §5)", long_about = None)]
    AddPivotTable(pivot::AddPivotTableCommand),
    /// Clears the pivot table at an anchor cell. Gated by the folder write-permission
    /// rules' `sheets-write` operation alone (issue rust-works/omni-dev#1798, ADR-0081
    /// §5).
    #[command(about = "Clears the pivot table at an anchor cell. Gated by the folder write-permission rules' sheets-write operation alone (issue rust-works/omni-dev#1798, ADR-0081 §5)", long_about = None)]
    DeletePivotTable(pivot::DeletePivotTableCommand),
    /// Lists the pivot tables in a spreadsheet, by anchor cell. Read-only and ungated,
    /// like `list-conditional-formats` (issue rust-works/omni-dev#1798).
    #[command(about = "Lists the pivot tables in a spreadsheet, by anchor cell. Read-only and ungated, like list-conditional-formats (issue rust-works/omni-dev#1798)", long_about = None)]
    ListPivotTables(pivot::ListPivotTablesCommand),
    /// Adds a banded range — alternating row or column colors. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1832, ADR-0082): presentation applied to a range, same
    /// reasoning as `unmerge-cells`/`clear-data-validation`.
    #[command(about = "Adds a banded range — alternating row or column colors. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1832, ADR-0082): presentation applied to a range, same reasoning as unmerge-cells/clear-data-validation", long_about = None)]
    AddBanding(banding::AddBandingCommand),
    /// Changes an existing banded range's range and/or colors. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1832, ADR-0082).
    #[command(about = "Changes an existing banded range's range and/or colors. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1832, ADR-0082)", long_about = None)]
    UpdateBanding(banding::UpdateBandingCommand),
    /// Removes a banded range. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1832, ADR-0082) — it
    /// removes presentation, not grid data.
    #[command(about = "Removes a banded range. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1832, ADR-0082) — it removes presentation, not grid data", long_about = None)]
    DeleteBanding(banding::DeleteBandingCommand),
    /// Lists the banded ranges in a spreadsheet. Read-only and ungated, like
    /// `list-protections` (issue rust-works/omni-dev#1832).
    #[command(about = "Lists the banded ranges in a spreadsheet. Read-only and ungated, like list-protections (issue rust-works/omni-dev#1832)", long_about = None)]
    ListBandings(banding::ListBandingsCommand),
    /// Adds a new outline group — the collapsible +/- grouping bar — over a span of
    /// rows or columns. Gated by the folder write-permission rules' `sheets-structure`
    /// operation (issue rust-works/omni-dev#1833, ADR-0084): the same reasoning as
    /// `add-banding`.
    #[command(about = "Adds a new outline group — the collapsible +/- grouping bar — over a span of rows or columns. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1833, ADR-0084): the same reasoning as add-banding", long_about = None)]
    AddDimensionGroup(dimension_group::AddDimensionGroupCommand),
    /// Changes an existing group's `collapsed` state. Gated by the folder
    /// write-permission rules' `sheets-structure` operation (issue
    /// rust-works/omni-dev#1833, ADR-0084).
    #[command(about = "Changes an existing group's collapsed state. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1833, ADR-0084)", long_about = None)]
    UpdateDimensionGroup(dimension_group::UpdateDimensionGroupCommand),
    /// Removes an outline group. Gated by the folder write-permission rules'
    /// `sheets-structure` operation (issue rust-works/omni-dev#1833, ADR-0084) — it
    /// removes presentation, not grid data.
    #[command(about = "Removes an outline group. Gated by the folder write-permission rules' sheets-structure operation (issue rust-works/omni-dev#1833, ADR-0084) — it removes presentation, not grid data", long_about = None)]
    DeleteDimensionGroup(dimension_group::DeleteDimensionGroupCommand),
    /// Lists the row and column outline groups in a spreadsheet. Read-only and ungated,
    /// like `list-bandings` (issue rust-works/omni-dev#1833).
    #[command(about = "Lists the row and column outline groups in a spreadsheet. Read-only and ungated, like list-bandings (issue rust-works/omni-dev#1833)", long_about = None)]
    ListDimensionGroups(dimension_group::ListDimensionGroupsCommand),
    /// Moves a range to a destination cell, clearing the source. Gated by **both** the
    /// folder write-permission rules' `sheets-write` and `sheets-structure` operations,
    /// whatever `--paste-type` names (issue rust-works/omni-dev#1839, ADR-0083 §4): the
    /// source is cleared in full regardless of what is pasted.
    #[command(about = "Moves a range to a destination cell, clearing the source. Gated by both the folder write-permission rules' sheets-write and sheets-structure operations, whatever --paste-type names (issue rust-works/omni-dev#1839, ADR-0083 §4): the source is cleared in full regardless of what is pasted", long_about = None)]
    CutPaste(paste::CutPasteCommand),
    /// Copies a range to a destination, spilling a larger source past the destination's
    /// end or repeating a smaller one to fill it. Gated by `--paste-type` (issue
    /// rust-works/omni-dev#1839, ADR-0083 §4): a value-only type needs `sheets-write`
    /// alone, a presentation-only type `sheets-structure` alone, and `normal` (the
    /// default) needs both.
    #[command(about = "Copies a range to a destination, spilling a larger source past the destination's end or repeating a smaller one to fill it. Gated by --paste-type (issue rust-works/omni-dev#1839, ADR-0083 §4): a value-only type needs sheets-write alone, a presentation-only type sheets-structure alone, and normal (the default) needs both", long_about = None)]
    CopyPaste(paste::CopyPasteCommand),
    /// Pastes delimited text into a range anchored at a destination cell, as if pasted
    /// from the clipboard (issue rust-works/omni-dev#1839, ADR-0083 §4).
    /// `delimiter`-form only. Same `--paste-type` gate mapping as `copy-paste`.
    #[command(about = "Pastes delimited text into a range anchored at a destination cell, as if pasted from the clipboard (issue rust-works/omni-dev#1839, ADR-0083 §4). delimiter-form only. Same --paste-type gate mapping as copy-paste", long_about = None)]
    PasteData(paste::PasteDataCommand),
    /// Extends a series from source cells into an adjacent destination, using Sheets'
    /// own pattern-detection heuristics. Gated by the folder write-permission rules'
    /// `sheets-write` operation (issue rust-works/omni-dev#1840, ADR-0083 §1) — it
    /// writes ordinary cell content, doing nothing a `sheets clear` followed by a
    /// `sheets write` could not already do under the same grant. The filled values can
    /// never be previewed; see `auto-fill --help`.
    #[command(about = "Extends a series from source cells into an adjacent destination, using Sheets' own pattern-detection heuristics. Gated by the folder write-permission rules' sheets-write operation (issue rust-works/omni-dev#1840, ADR-0083 §1) — it writes ordinary cell content, doing nothing a sheets clear followed by a sheets write could not already do under the same grant. The filled values can never be previewed; see auto-fill --help", long_about = None)]
    AutoFill(auto_fill::AutoFillCommand),
    /// Reorders rows in a range by one or more column keys. Gated by the folder
    /// `sheets-write` **and** `sheets-structure` operations (issue
    /// rust-works/omni-dev#1842, rust-works/omni-dev#1870, ADR-0083 §§3, 5) — a sorted
    /// row was measured carrying its formatting, notes and data-validation rules with
    /// it.
    #[command(about = "Reorders rows in a range by one or more column keys. Gated by the folder sheets-write and sheets-structure operations (issue rust-works/omni-dev#1842, rust-works/omni-dev#1870, ADR-0083 §§3, 5) — a sorted row was measured carrying its formatting, notes and data-validation rules with it", long_about = None)]
    SortRange(sort_range::SortRangeCommand),
    /// Shuffles the row order within a range into an order chosen by the server. Gated
    /// by the folder `sheets-write` **and** `sheets-structure` operations (issue
    /// rust-works/omni-dev#1845, ADR-0083 §§3, 5, 6) — a reordered row was measured
    /// carrying its formatting, notes and data-validation rules with it. The resulting
    /// order can never be previewed; see `randomize-range --help`.
    #[command(about = "Shuffles the row order within a range into an order chosen by the server. Gated by the folder sheets-write and sheets-structure operations (issue rust-works/omni-dev#1845, ADR-0083 §§3, 5, 6) — a reordered row was measured carrying its formatting, notes and data-validation rules with it. The resulting order can never be previewed; see randomize-range --help", long_about = None)]
    RandomizeRange(randomize_range::RandomizeRangeCommand),
    /// Splits a single column's delimited text across the adjacent columns to its
    /// right. Gated by the folder `sheets-write` **and** `sheets-structure` operations
    /// (issue rust-works/omni-dev#1843, ADR-0083 §§1, 5) — it writes ordinary cell
    /// content, and carries the source cell's formatting into the columns it spills
    /// into. How many columns the split needs, and the values it writes, can never be
    /// previewed; see `text-to-columns --help`.
    #[command(about = "Splits a single column's delimited text across the adjacent columns to its right. Gated by the folder sheets-write and sheets-structure operations (issue rust-works/omni-dev#1843, ADR-0083 §§1, 5) — it writes ordinary cell content, and carries the source cell's formatting into the columns it spills into. How many columns the split needs, and the values it writes, can never be previewed; see text-to-columns --help", long_about = None)]
    TextToColumns(text_to_columns::TextToColumnsCommand),
    /// Trims whitespace in every cell of a range, or of a whole sheet. Trimming strips
    /// leading and trailing whitespace **and collapses each internal run to a single
    /// space** (measured live: `a   b` becomes `a b`); text that trims to something
    /// starting `=` or `+` stays a string and is not reinterpreted as a formula. Gated
    /// by the folder `sheets-write` operation (issue rust-works/omni-dev#1844, ADR-0083
    /// §1) — it rewrites ordinary cell content in place, doing nothing a `sheets clear`
    /// followed by a `sheets write` of the same range could not already do under the
    /// same grant. Sheets owns the trim rule, so `--dry-run` reports the non-blank
    /// cells that may change, never the ones that will.
    #[command(about = "Trims whitespace in every cell of a range, or of a whole sheet. Trimming strips leading and trailing whitespace and collapses each internal run to a single space (measured live: a   b becomes a b); text that trims to something starting = or + stays a string and is not reinterpreted as a formula. Gated by the folder sheets-write operation (issue rust-works/omni-dev#1844, ADR-0083 §1) — it rewrites ordinary cell content in place, doing nothing a sheets clear followed by a sheets write of the same range could not already do under the same grant. Sheets owns the trim rule, so --dry-run reports the non-blank cells that may change, never the ones that will", long_about = None)]
    TrimWhitespace(trim_whitespace::TrimWhitespaceCommand),
    /// Removes duplicate row cells within a bounded range. Gated by the folder
    /// `sheets-delete` operation rather than `sheets-write` (issue
    /// rust-works/omni-dev#1844, ADR-0083 §2): cells inside the selected range are
    /// removed and its survivors shift up. The API selects the rows — keeping the first
    /// instance of each duplicate, counting rows that differ only in case, formatting
    /// or formulas, and removing filter-hidden rows — so `--dry-run` states that rule
    /// instead of listing rows it cannot vouch for. Blank rows duplicate one another,
    /// so a range extending past the data can remove every blank row but the first.
    /// Columns outside the range stay in place, so a narrow range can misalign records.
    #[command(about = "Removes duplicate row cells within a bounded range. Gated by the folder sheets-delete operation rather than sheets-write (issue rust-works/omni-dev#1844, ADR-0083 §2): cells inside the selected range are removed and its survivors shift up. The API selects the rows — keeping the first instance of each duplicate, counting rows that differ only in case, formatting or formulas, and removing filter-hidden rows — so --dry-run states that rule instead of listing rows it cannot vouch for. Blank rows duplicate one another, so a range extending past the data can remove every blank row but the first. Columns outside the range stay in place, so a narrow range can misalign records", long_about = None)]
    DeleteDuplicates(delete_duplicates::DeleteDuplicatesCommand),
}

impl SheetsCommand {
    /// Runs the command against the shared Drive client resolved by the
    /// parent `DriveCommand::execute`.
    ///
    /// Each leaf derives its own `SheetsClient` from that Drive client so the
    /// two hosts share one OAuth session — see
    /// [`crate::drive::sheets::client::SheetsClient::from_drive_client`].
    pub async fn execute(self, client: &DriveClient) -> Result<()> {
        match self.command {
            SheetsSubcommands::Info(cmd) => cmd.execute(client).await,
            SheetsSubcommands::Read(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ReadCellFormat(cmd) => cmd.execute(client).await,
            SheetsSubcommands::Write(cmd) => cmd.execute(client).await,
            SheetsSubcommands::Append(cmd) => cmd.execute(client).await,
            SheetsSubcommands::Clear(cmd) => cmd.execute(client).await,
            SheetsSubcommands::FindReplace(cmd) => cmd.execute(client).await,
            SheetsSubcommands::Create(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::RenameSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::InsertRows(cmd) => cmd.execute(client).await,
            SheetsSubcommands::InsertColumns(cmd) => cmd.execute(client).await,
            SheetsSubcommands::InsertRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::MoveRows(cmd) => cmd.execute(client).await,
            SheetsSubcommands::MoveColumns(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteRows(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteColumns(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DuplicateSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ReorderSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::HideSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ShowSheet(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateSheetProperties(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateWorkbookProperties(cmd) => cmd.execute(client).await,
            SheetsSubcommands::FormatCells(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateBorders(cmd) => cmd.execute(client).await,
            SheetsSubcommands::MergeCells(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UnmergeCells(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AutoResizeDimension(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateDimensionProperties(cmd) => cmd.execute(client).await,
            SheetsSubcommands::SetDataValidation(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ClearDataValidation(cmd) => cmd.execute(client).await,
            SheetsSubcommands::SetDeveloperMetadata(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteDeveloperMetadata(cmd) => cmd.execute(client).await,
            SheetsSubcommands::SearchDeveloperMetadata(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ProtectRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateProtection(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UnprotectRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListProtections(cmd) => cmd.execute(client).await,
            SheetsSubcommands::SetBasicFilter(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ClearBasicFilter(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddFilterView(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateFilterView(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteFilterView(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListFilterViews(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddConditionalFormat(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateConditionalFormat(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteConditionalFormat(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListConditionalFormats(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddNamedRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateNamedRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteNamedRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListNamedRanges(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddChart(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateChart(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteChart(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListCharts(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddSlicer(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateSlicer(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteSlicer(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListSlicers(cmd) => cmd.execute(client).await,
            SheetsSubcommands::MoveChart(cmd) => cmd.execute(client).await,
            SheetsSubcommands::MoveSlicer(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateChartBorder(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddPivotTable(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeletePivotTable(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListPivotTables(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddBanding(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateBanding(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteBanding(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListBandings(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AddDimensionGroup(cmd) => cmd.execute(client).await,
            SheetsSubcommands::UpdateDimensionGroup(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteDimensionGroup(cmd) => cmd.execute(client).await,
            SheetsSubcommands::ListDimensionGroups(cmd) => cmd.execute(client).await,
            SheetsSubcommands::CutPaste(cmd) => cmd.execute(client).await,
            SheetsSubcommands::CopyPaste(cmd) => cmd.execute(client).await,
            SheetsSubcommands::PasteData(cmd) => cmd.execute(client).await,
            SheetsSubcommands::AutoFill(cmd) => cmd.execute(client).await,
            SheetsSubcommands::SortRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::RandomizeRange(cmd) => cmd.execute(client).await,
            SheetsSubcommands::TextToColumns(cmd) => cmd.execute(client).await,
            SheetsSubcommands::TrimWhitespace(cmd) => cmd.execute(client).await,
            SheetsSubcommands::DeleteDuplicates(cmd) => cmd.execute(client).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::drive::auth::{DriveCredentials, DriveGrantedScopes};
    use crate::utils::secret::Secret;

    fn test_credentials() -> DriveCredentials {
        DriveCredentials {
            client_id: "client-1".to_string(),
            client_secret: Secret::new("secret-1"),
            refresh_token: Secret::new("refresh-1"),
            scope: DriveGrantedScopes::READONLY,
        }
    }

    async fn client_with_bootstrapped_token(server: &wiremock::MockServer) -> DriveClient {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token": "test-token",
                    "expires_in": 3600,
                })),
            )
            .mount(server)
            .await;

        let mut client = DriveClient::new(&server.uri(), &test_credentials()).unwrap();
        crate::drive::client::test_support::replace_session(
            &mut client,
            &test_credentials(),
            &format!("{}/token", server.uri()),
        );
        client
    }

    /// Runs `cmd` through `SheetsCommand::execute`'s own dispatch match,
    /// rather than calling the leaf's `execute` directly — every other test
    /// in this crate's `drive sheets` leaves does the latter, which leaves
    /// this match's arms themselves uncovered (issue #1796's coverage
    /// review, PR #1811).
    async fn dispatch(command: SheetsSubcommands, client: &DriveClient) -> Result<()> {
        SheetsCommand { command }.execute(client).await
    }

    #[tokio::test]
    async fn the_named_range_dispatch_arms_reach_their_leaf_commands() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so every mutating leaf below is `Blocked` by default
        // policy — enough to reach and return from the leaf without a
        // lease or a workbook fetch, which a `Blocked` verdict never gets
        // to. `list-named-ranges` is ungated, so it goes further and
        // actually fetches the (named-range-free) workbook.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v4/spreadsheets/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "spreadsheetId": "sheet-1",
                    "properties": {"title": "Budget"},
                    "sheets": [{"properties": {"sheetId": 0, "title": "Sheet1"}}],
                    "namedRanges": [],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::AddNamedRange(named_range::AddNamedRangeCommand {
                spreadsheet_id: "sheet-1".to_string(),
                name: "Foo".to_string(),
                range: Some("A1:A5".to_string()),
                sheet: Some("Q1".to_string()),
                whole_sheet: false,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::UpdateNamedRange(named_range::UpdateNamedRangeCommand {
                spreadsheet_id: "sheet-1".to_string(),
                name: Some("Foo".to_string()),
                id: None,
                new_name: Some("Bar".to_string()),
                range: None,
                sheet: None,
                whole_sheet: false,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeleteNamedRange(named_range::DeleteNamedRangeCommand {
                spreadsheet_id: "sheet-1".to_string(),
                name: Some("Foo".to_string()),
                id: None,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::ListNamedRanges(named_range::ListNamedRangesCommand {
                spreadsheet_id: "sheet-1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::AutoFill(auto_fill::AutoFillCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Sheet1".to_string()),
                range: Some("A1:A3".to_string()),
                source: None,
                dimension: None,
                fill_length: None,
                alternate_series: false,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::RandomizeRange(randomize_range::RandomizeRangeCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Sheet1".to_string()),
                range: "A1:A3".to_string(),
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::TextToColumns(text_to_columns::TextToColumnsCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Sheet1".to_string()),
                source: "A1:A3".to_string(),
                delimiter: text_to_columns::DelimiterArg::Comma,
                custom_delimiter: None,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::TrimWhitespace(trim_whitespace::TrimWhitespaceCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Sheet1".to_string()),
                range: Some("A1:C9".to_string()),
                whole_sheet: false,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeleteDuplicates(delete_duplicates::DeleteDuplicatesCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Sheet1".to_string()),
                range: "A1:C9".to_string(),
                comparison_column: vec![0],
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_chart_and_slicer_dispatch_arms_reach_their_leaf_commands() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // Same trick as `the_named_range_dispatch_arms_reach_their_leaf_commands`
        // above: with no write-permission rules configured, every mutating
        // chart/slicer leaf is `Blocked` by default policy, which returns
        // `Ok(())` without a lease or a `batchUpdate`. `list-charts` and
        // `list-slicers` are ungated, so they go on to fetch the workbook.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v4/spreadsheets/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "spreadsheetId": "sheet-1",
                    "properties": {"title": "Budget"},
                    "sheets": [{"properties": {"sheetId": 0, "title": "Sheet1"}}],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::AddChart(Box::new(embedded_object::AddChartCommand {
                spreadsheet_id: "sheet-1".to_string(),
                chart_type: "column".to_string(),
                domain: "A1:A10".to_string(),
                series: vec!["B1:B10".to_string()],
                sheet: Some("Sheet1".to_string()),
                title: None,
                subtitle: None,
                legend: None,
                stacked: None,
                header_count: None,
                horizontal_axis_title: None,
                vertical_axis_title: None,
                pie_hole: None,
                anchor: Some("E2".to_string()),
                offset_x: None,
                offset_y: None,
                width: None,
                height: None,
                new_sheet: false,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            })),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::UpdateChart(Box::new(embedded_object::UpdateChartCommand {
                spreadsheet_id: "sheet-1".to_string(),
                chart_id: 1,
                chart_type: None,
                domain: None,
                series: Vec::new(),
                sheet: None,
                title: Some("New title".to_string()),
                subtitle: None,
                legend: None,
                stacked: None,
                header_count: None,
                horizontal_axis_title: None,
                vertical_axis_title: None,
                pie_hole: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            })),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeleteChart(embedded_object::DeleteChartCommand {
                spreadsheet_id: "sheet-1".to_string(),
                chart_id: 1,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::ListCharts(embedded_object::ListChartsCommand {
                spreadsheet_id: "sheet-1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::AddSlicer(embedded_object::AddSlicerCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Sheet1".to_string()),
                range: "A1:D10".to_string(),
                column: 1,
                hide_values: vec!["Closed".to_string()],
                title: None,
                apply_to_pivot_tables: None,
                anchor: "F2".to_string(),
                offset_x: None,
                offset_y: None,
                width: None,
                height: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::UpdateSlicer(embedded_object::UpdateSlicerCommand {
                spreadsheet_id: "sheet-1".to_string(),
                slicer_id: 1,
                sheet: None,
                range: None,
                column: None,
                hide_values: Vec::new(),
                clear_criteria: true,
                title: None,
                apply_to_pivot_tables: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeleteSlicer(embedded_object::DeleteSlicerCommand {
                spreadsheet_id: "sheet-1".to_string(),
                slicer_id: 1,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::ListSlicers(embedded_object::ListSlicersCommand {
                spreadsheet_id: "sheet-1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::MoveChart(embedded_object::MoveChartCommand {
                spreadsheet_id: "sheet-1".to_string(),
                chart_id: 1,
                sheet: None,
                anchor: Some("F2".to_string()),
                offset_x: None,
                offset_y: None,
                width: None,
                height: None,
                new_sheet: false,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::MoveSlicer(embedded_object::MoveSlicerCommand {
                spreadsheet_id: "sheet-1".to_string(),
                slicer_id: 1,
                sheet: None,
                anchor: Some("F2".to_string()),
                offset_x: None,
                offset_y: None,
                width: None,
                height: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::UpdateChartBorder(embedded_object::UpdateChartBorderCommand {
                spreadsheet_id: "sheet-1".to_string(),
                chart_id: 1,
                color: Some("#4A86E8".to_string()),
                clear: false,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_pivot_table_dispatch_arms_reach_their_leaf_commands() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so both mutating leaves below are `Blocked` by default
        // policy — enough to reach and return from the leaf without a
        // lease or a workbook fetch. `list-pivot-tables` is ungated, so it
        // goes further and actually fetches the (pivot-table-free)
        // workbook.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v4/spreadsheets/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "spreadsheetId": "sheet-1",
                    "properties": {"title": "Budget"},
                    "sheets": [{"properties": {"sheetId": 0, "title": "Sheet1"}}],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::AddPivotTable(pivot::AddPivotTableCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: "Q1".to_string(),
                anchor: "A1".to_string(),
                source: "A1:B10".to_string(),
                rows: vec!["0".to_string()],
                columns: Vec::new(),
                values: vec!["1:sum".to_string()],
                filters: Vec::new(),
                value_layout: None,
                no_totals: false,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeletePivotTable(pivot::DeletePivotTableCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: "Q1".to_string(),
                anchor: "A1".to_string(),
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::ListPivotTables(pivot::ListPivotTablesCommand {
                spreadsheet_id: "sheet-1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_banding_dispatch_arms_reach_their_leaf_commands() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so every mutating leaf below is `Blocked` by default
        // policy — enough to reach and return from the leaf without a
        // lease or a workbook fetch. `list-bandings` is ungated, so it
        // goes further and actually fetches the (banding-free) workbook.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v4/spreadsheets/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "spreadsheetId": "sheet-1",
                    "properties": {"title": "Budget"},
                    "sheets": [{"properties": {"sheetId": 0, "title": "Sheet1"}}],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::AddBanding(banding::AddBandingCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: "Sheet1".to_string(),
                range: "A1:D10".to_string(),
                axis: banding::BandingAxisArg::Rows,
                header_color: None,
                first_band_color: "#FFFFFF".to_string(),
                second_band_color: "#EEEEEE".to_string(),
                footer_color: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::UpdateBanding(banding::UpdateBandingCommand {
                spreadsheet_id: "sheet-1".to_string(),
                banded_range_id: 1,
                sheet: None,
                range: None,
                axis: banding::BandingAxisArg::Rows,
                header_color: Some("#000000".to_string()),
                first_band_color: None,
                second_band_color: None,
                footer_color: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeleteBanding(banding::DeleteBandingCommand {
                spreadsheet_id: "sheet-1".to_string(),
                banded_range_id: 1,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::ListBandings(banding::ListBandingsCommand {
                spreadsheet_id: "sheet-1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_dimension_group_dispatch_arms_reach_their_leaf_commands() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so every mutating leaf below is `Blocked` by default
        // policy — enough to reach and return from the leaf without a
        // lease or a workbook fetch. `list-dimension-groups` is ungated,
        // so it goes further and actually fetches the (group-free)
        // workbook.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v4/spreadsheets/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "spreadsheetId": "sheet-1",
                    "properties": {"title": "Budget"},
                    "sheets": [{"properties": {"sheetId": 0, "title": "Sheet1"}}],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::AddDimensionGroup(dimension_group::AddDimensionGroupCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: "Sheet1".to_string(),
                dimension: crate::cli::drive::sheets::format::DimensionArg::Rows,
                start: 1,
                end: 5,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::UpdateDimensionGroup(dimension_group::UpdateDimensionGroupCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: "Sheet1".to_string(),
                dimension: crate::cli::drive::sheets::format::DimensionArg::Rows,
                start: 1,
                end: 5,
                depth: None,
                collapsed: true,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::DeleteDimensionGroup(dimension_group::DeleteDimensionGroupCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: "Sheet1".to_string(),
                dimension: crate::cli::drive::sheets::format::DimensionArg::Rows,
                start: 1,
                end: 5,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table
                },
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::ListDimensionGroups(dimension_group::ListDimensionGroupsCommand {
                spreadsheet_id: "sheet-1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_update_sheet_properties_dispatch_arm_reaches_its_leaf_command() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so the mutating leaf below is `Blocked` by default
        // policy — enough to reach and return from the leaf without a
        // lease or a workbook fetch, which a `Blocked` verdict never gets
        // to (same trick as `the_banding_dispatch_arms_reach_their_leaf_commands`).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::UpdateSheetProperties(structure::UpdateSheetPropertiesCommand {
                target: structure::SpreadsheetIdArg {
                    spreadsheet_id: "sheet-1".to_string(),
                },
                sheet: "Q1".to_string(),
                freeze_rows: Some(1),
                freeze_columns: None,
                tab_color: Some("#FF8800".to_string()),
                clear_tab_color: false,
                right_to_left: None,
                hide_gridlines: None,
                write: crate::cli::drive::helpers::StructureWriteArgs {
                    dry_run: true,
                    lease: no_lease(),
                    output: crate::cli::drive::format::OutputFormat::Table,
                },
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_find_replace_dispatch_arm_reaches_its_leaf_command() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so the mutating leaf below is `Blocked` by default
        // policy — enough to reach and return from the leaf without a
        // lease or a workbook fetch, which a `Blocked` verdict never gets
        // to (same trick as `the_update_sheet_properties_dispatch_arm_reaches_its_leaf_command`).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;

        assert!(dispatch(
            SheetsSubcommands::FindReplace(find_replace::FindReplaceCommand {
                spreadsheet_id: "sheet-1".to_string(),
                find: "foo".to_string(),
                replacement: "bar".to_string(),
                range: Some("Q1!A1:B2".to_string()),
                sheet: None,
                whole_sheet: false,
                all_sheets: false,
                match_case: false,
                match_entire_cell: false,
                search_by_regex: false,
                include_formulas: false,
                dry_run: true,
                lease: crate::cli::drive::helpers::LeaseTokenArg { lease: None },
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_paste_dispatch_arms_reach_their_leaf_commands() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        // No write-permission rules are configured (an unconfigured
        // account), so every leaf below is `Blocked` by default policy —
        // enough to reach and return from the leaf without a lease or a
        // workbook fetch, which a `Blocked` verdict never gets to (same
        // trick as `the_banding_dispatch_arms_reach_their_leaf_commands`).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "sheet-1",
                    "name": "Budget",
                    "mimeType": crate::drive::types::GOOGLE_SHEET_MIME_TYPE,
                    "parents": ["folder-1"],
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/drive/v3/files/folder-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "folder-1",
                    "name": "folder-1",
                    "mimeType": "application/vnd.google-apps.folder",
                    "parents": [],
                })),
            )
            .mount(&server)
            .await;

        fn no_lease() -> crate::cli::drive::helpers::LeaseTokenArg {
            crate::cli::drive::helpers::LeaseTokenArg { lease: None }
        }

        assert!(dispatch(
            SheetsSubcommands::CutPaste(paste::CutPasteCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Q1".to_string()),
                source: "A1:B2".to_string(),
                destination: "D1".to_string(),
                paste_type: paste::PasteTypeArg::Normal,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        assert!(dispatch(
            SheetsSubcommands::CopyPaste(paste::CopyPasteCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Q1".to_string()),
                source: "A1:B2".to_string(),
                destination: "D1:E2".to_string(),
                paste_type: paste::PasteTypeArg::Normal,
                orientation: paste::OrientationArg::Normal,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.tsv");
        std::fs::write(&path, "1\t2").unwrap();
        assert!(dispatch(
            SheetsSubcommands::PasteData(paste::PasteDataCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Q1".to_string()),
                destination: "A1".to_string(),
                data: None,
                data_file: Some(path.to_str().unwrap().to_string()),
                delimiter: "\t".to_string(),
                paste_type: paste::PasteTypeArg::Values,
                dry_run: true,
                lease: no_lease(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn the_read_cell_format_dispatch_arm_reaches_its_leaf_command() {
        let guard = crate::drive::test_support::EnvGuard::take();
        let _dir = guard.clear_credentials();

        let server = wiremock::MockServer::start().await;
        let client = client_with_bootstrapped_token(&server).await;
        std::env::set_var(crate::drive::sheets::client::SHEETS_API_URL, server.uri());
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v4/spreadsheets/sheet-1"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "spreadsheetId": "sheet-1",
                    "properties": {"title": "Budget"},
                    "sheets": [{
                        "properties": {"sheetId": 0, "title": "Q1"},
                        "data": [{
                            "startRow": 0,
                            "startColumn": 0,
                            "rowData": [{"values": [{}]}],
                        }],
                    }],
                })),
            )
            .mount(&server)
            .await;

        assert!(dispatch(
            SheetsSubcommands::ReadCellFormat(cell_format::ReadCellFormatCommand {
                spreadsheet_id: "sheet-1".to_string(),
                sheet: Some("Q1".to_string()),
                range: "A1".to_string(),
                output: crate::cli::drive::format::OutputFormat::Table,
            }),
            &client,
        )
        .await
        .is_ok());
    }
}
