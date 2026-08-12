use std::{ffi::c_void, mem::size_of, path::PathBuf, thread};

use quick_preview::{
    document::FormatDocument, encoding::TextEncoding, history::EditCommand,
    web_message::WebEditMessage, DocumentSession, PreviewError,
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
    core::{w, Error, Result, PCWSTR, PWSTR},
    Win32::{
        Foundation::{E_POINTER, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{D2D1_COLOR_F, D2D_RECT_F, D2D_SIZE_U},
                *,
            },
            DirectWrite::*,
            Gdi::{
                BeginPaint, EndPaint, GetSysColorBrush, InvalidateRect, UpdateWindow, COLOR_WINDOW,
                PAINTSTRUCT,
            },
        },
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Controls::{
                Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW},
                EM_SETSEL,
            },
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                GetKeyState, SetFocus, VK_CONTROL, VK_DOWN, VK_F2, VK_LEFT, VK_RETURN, VK_RIGHT,
                VK_UP,
            },
            Shell::{
                DefSubclassProc, DragAcceptFiles, DragFinish, DragQueryFileW, SetWindowSubclass,
                ShellExecuteW, HDROP,
            },
            WindowsAndMessaging::*,
        },
    },
};

const WM_DOCUMENT_READY: u32 = WM_APP + 1;
const WM_WEB_MESSAGE: u32 = WM_APP + 2;
const EDIT_ID: i32 = 4101;
const CELL_WIDTH: f32 = 160.0;
const CELL_HEIGHT: f32 = 28.0;
const HEADER_WIDTH: f32 = 58.0;
const HEADER_HEIGHT: f32 = 30.0;

pub fn run() -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            lpszClassName: w!("QuickPreviewWindow"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class.lpszClassName,
            w!("QuickPreview"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1120,
            760,
            None,
            None,
            Some(class.hInstance),
            None,
        )?;
        let state = Box::new(AppState::new(hwnd)?);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        DragAcceptFiles(hwnd, true);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);

        if let Some(path) = std::env::args_os().nth(1) {
            begin_open(hwnd, PathBuf::from(path));
        }
        let mut message = MSG::default();
        loop {
            let value = GetMessageW(&mut message, None, 0, 0).0;
            if value == -1 {
                return Err(Error::from_thread());
            }
            if value == 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Ok(())
    }
}

struct AppState {
    hwnd: HWND,
    document: Option<DocumentSession>,
    renderer: GridRenderer,
    webview: Option<WebPreview>,
    selected_row: usize,
    selected_column: usize,
    first_row: usize,
    first_column: usize,
    editor: Option<HWND>,
    loading: bool,
}

impl AppState {
    fn new(hwnd: HWND) -> Result<Self> {
        Ok(Self {
            hwnd,
            document: None,
            renderer: GridRenderer::new(hwnd)?,
            webview: None,
            selected_row: 0,
            selected_column: 0,
            first_row: 0,
            first_column: 0,
            editor: None,
            loading: false,
        })
    }

    fn set_document(&mut self, result: quick_preview::Result<DocumentSession>) {
        self.loading = false;
        match result {
            Ok(document) => {
                self.webview = None;
                self.document = Some(document);
                self.selected_row = 0;
                self.selected_column = 0;
                self.first_row = 0;
                self.first_column = 0;
                self.refresh_view();
                self.update_title();
            }
            Err(error) => show_error(self.hwnd, &error.to_string()),
        }
    }

    fn refresh_view(&mut self) {
        let Some(document) = self.document.as_ref() else {
            return;
        };
        match &document.document {
            FormatDocument::Delimited(_) => {
                self.webview = None;
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            }
            FormatDocument::Markdown(markdown) => {
                self.show_web(markdown.preview_html(document.revision))
            }
            FormatDocument::Html(html) => self.show_web(html.preview_html(document.revision)),
        }
    }

    fn show_web(&mut self, html: String) {
        if self.webview.is_none() {
            match WebPreview::create(self.hwnd) {
                Ok(webview) => self.webview = Some(webview),
                Err(error) => {
                    show_error(self.hwnd, &format!("WebView2 Runtimeを開始できません。Evergreen Runtimeをインストールしてください。\n\n{error}"));
                    return;
                }
            }
        }
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.navigate_to_string(&html) {
                show_error(self.hwnd, &error.to_string());
            }
        }
    }

    fn update_title(&self) {
        let title = self
            .document
            .as_ref()
            .map(|doc| {
                format!(
                    "{}{} — QuickPreview",
                    if doc.dirty { "*" } else { "" },
                    doc.path.file_name().unwrap_or_default().to_string_lossy()
                )
            })
            .unwrap_or_else(|| "QuickPreview".into());
        let wide = wide(&title);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, PCWSTR(wide.as_ptr()));
        }
    }

    fn save(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        match document.save() {
            Ok(()) => self.update_title(),
            Err(PreviewError::UnrepresentableShiftJis) => unsafe {
                if MessageBoxW(
                    Some(self.hwnd),
                    w!("編集内容をShift_JISで表現できません。UTF-8へ変換して保存しますか？"),
                    w!("QuickPreview"),
                    MB_YESNO | MB_ICONQUESTION,
                ) == IDYES
                {
                    document.encoding.encoding = TextEncoding::Utf8;
                    document.encoding.bom = false;
                    match &mut document.document {
                        FormatDocument::Delimited(d) => {
                            d.encoding = document.encoding;
                        }
                        FormatDocument::Markdown(d) => {
                            d.encoding = document.encoding;
                        }
                        FormatDocument::Html(d) => {
                            d.encoding = document.encoding;
                        }
                    }
                    if let Err(error) = document.save() {
                        show_error(self.hwnd, &error.to_string());
                    } else {
                        self.update_title();
                    }
                }
            },
            Err(error) => show_error(self.hwnd, &error.to_string()),
        }
    }

    fn begin_cell_edit(&mut self) {
        if self.editor.is_some() {
            return;
        }
        let Some(DocumentSession {
            document: FormatDocument::Delimited(grid),
            ..
        }) = self.document.as_ref()
        else {
            return;
        };
        let value = match grid.cell(self.selected_row, self.selected_column) {
            Ok(value) => value,
            Err(_) => return,
        };
        let rect = self.cell_rect(self.selected_row, self.selected_column);
        let text = wide(&value);
        unsafe {
            if let Ok(edit) = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                PCWSTR(text.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                Some(self.hwnd),
                Some(HMENU(EDIT_ID as *mut c_void)),
                None,
                None,
            ) {
                let _ = SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                let _ = SetWindowSubclass(edit, Some(edit_subclass_proc), 1, 0);
                let _ = SetFocus(Some(edit));
                self.editor = Some(edit);
            }
        }
    }

    fn commit_cell_edit(&mut self) {
        let Some(edit) = self.editor.take() else {
            return;
        };
        let text = window_text(edit);
        unsafe {
            let _ = DestroyWindow(edit);
        }
        let Some(document) = self.document.as_mut() else {
            return;
        };
        if let FormatDocument::Delimited(grid) = &mut document.document {
            match grid.edit_cell(self.selected_row, self.selected_column, text.clone()) {
                Ok(before) if before != text => {
                    document.undo_stack.push(EditCommand::Cell {
                        row: self.selected_row,
                        column: self.selected_column,
                        before,
                        after: text,
                    });
                    document.mark_edited();
                    self.update_title();
                }
                Ok(_) => {}
                Err(error) => show_error(self.hwnd, &error.to_string()),
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn handle_web_message(&mut self, json: String) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let value: serde_json::Value = match serde_json::from_str(&json) {
            Ok(value) => value,
            Err(_) => return,
        };
        if value.get("type").and_then(|v| v.as_str()) == Some("openLink") {
            let revision = value.get("documentRevision").and_then(|v| v.as_u64());
            let url = value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if revision == Some(document.revision)
                && (url.starts_with("https://") || url.starts_with("http://"))
            {
                let prompt = wide(&format!("既定のブラウザーで開きますか？\n\n{url}"));
                unsafe {
                    if MessageBoxW(
                        Some(self.hwnd),
                        PCWSTR(prompt.as_ptr()),
                        w!("QuickPreview"),
                        MB_YESNO | MB_ICONQUESTION,
                    ) == IDYES
                    {
                        let target = wide(url);
                        let _ = ShellExecuteW(
                            Some(self.hwnd),
                            w!("open"),
                            PCWSTR(target.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        );
                    }
                }
            }
            return;
        }
        let message = match WebEditMessage::parse(&json, document.revision) {
            Ok(message) => message,
            Err(error) => {
                show_error(self.hwnd, &error.to_string());
                return;
            }
        };
        let edit = match &mut document.document {
            FormatDocument::Markdown(markdown) => {
                let start = markdown
                    .blocks
                    .iter()
                    .find(|b| b.id == message.node_id)
                    .map(|b| b.range.start)
                    .unwrap_or(0);
                markdown
                    .edit_block(message.node_id, &message.text)
                    .map(|before| EditCommand::Source {
                        start,
                        before,
                        after: message.text,
                    })
            }
            FormatDocument::Html(html) => {
                let start = html
                    .nodes
                    .iter()
                    .find(|n| n.id == message.node_id)
                    .map(|n| n.range.start)
                    .unwrap_or(0);
                html.edit_text(message.node_id, &message.text)
                    .map(|before| {
                        let after = html.source()[start..]
                            .split('<')
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        EditCommand::Source {
                            start,
                            before,
                            after,
                        }
                    })
            }
            _ => return,
        };
        match edit {
            Ok(command) => {
                if matches!(&command, EditCommand::Source { before, after, .. } if before == after)
                {
                    self.refresh_view();
                    return;
                }
                document.undo_stack.push(command);
                document.mark_edited();
                self.update_title();
                self.refresh_view();
            }
            Err(error) => show_error(self.hwnd, &error.to_string()),
        }
    }

    fn undo(&mut self, redo: bool) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        let command = if redo {
            document.undo_stack.redo()
        } else {
            document.undo_stack.undo()
        };
        let Some(command) = command else { return };
        let result = match command {
            EditCommand::Cell {
                row,
                column,
                before,
                after,
            } => {
                let value = if redo { after } else { before };
                if let FormatDocument::Delimited(grid) = &mut document.document {
                    grid.set_cell_without_history(row, column, value)
                } else {
                    Ok(())
                }
            }
            EditCommand::Source {
                start,
                before,
                after,
            } => {
                let (expected, replacement) = if redo {
                    (before, after)
                } else {
                    (after, before)
                };
                match &mut document.document {
                    FormatDocument::Markdown(doc) => doc.replace_at(start, &expected, &replacement),
                    FormatDocument::Html(doc) => doc.replace_at(start, &expected, &replacement),
                    _ => Ok(()),
                }
            }
        };
        if let Err(error) = result {
            show_error(self.hwnd, &error.to_string());
        } else {
            document.mark_edited();
            self.update_title();
            self.refresh_view();
        }
    }

    fn cell_rect(&self, row: usize, column: usize) -> RECT {
        let x = HEADER_WIDTH + ((column.saturating_sub(self.first_column)) as f32 * CELL_WIDTH);
        let y = HEADER_HEIGHT + ((row.saturating_sub(self.first_row)) as f32 * CELL_HEIGHT);
        RECT {
            left: x as i32,
            top: y as i32,
            right: (x + CELL_WIDTH) as i32,
            bottom: (y + CELL_HEIGHT) as i32,
        }
    }
}

struct GridRenderer {
    _factory: ID2D1Factory,
    target: ID2D1HwndRenderTarget,
    text_format: IDWriteTextFormat,
    text: ID2D1SolidColorBrush,
    line: ID2D1SolidColorBrush,
    header: ID2D1SolidColorBrush,
    selection: ID2D1SolidColorBrush,
}

impl GridRenderer {
    fn new(hwnd: HWND) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let size = client_size(hwnd);
            let props = D2D1_RENDER_TARGET_PROPERTIES::default();
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let target = factory.CreateHwndRenderTarget(&props, &hwnd_props)?;
            let write_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let text_format = write_factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("ja-JP"),
            )?;
            let brush = |color: D2D1_COLOR_F| target.CreateSolidColorBrush(&color, None);
            Ok(Self {
                text: brush(color(0.10, 0.10, 0.10, 1.0))?,
                line: brush(color(0.78, 0.78, 0.78, 1.0))?,
                header: brush(color(0.93, 0.94, 0.95, 1.0))?,
                selection: brush(color(0.78, 0.88, 1.0, 1.0))?,
                _factory: factory,
                target,
                text_format,
            })
        }
    }

    fn resize(&self, hwnd: HWND) {
        unsafe {
            let _ = self.target.Resize(&client_size(hwnd));
        }
    }

    fn draw(&self, state: &AppState) {
        unsafe {
            self.target.BeginDraw();
            self.target.Clear(Some(&color(1.0, 1.0, 1.0, 1.0)));
            if let Some(DocumentSession {
                document: FormatDocument::Delimited(grid),
                ..
            }) = state.document.as_ref()
            {
                let size = client_size(state.hwnd);
                let rows = ((size.height as f32 - HEADER_HEIGHT) / CELL_HEIGHT)
                    .ceil()
                    .max(0.0) as usize
                    + 1;
                let columns = ((size.width as f32 - HEADER_WIDTH) / CELL_WIDTH)
                    .ceil()
                    .max(0.0) as usize
                    + 1;
                self.target.FillRectangle(
                    &D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: size.width as f32,
                        bottom: HEADER_HEIGHT,
                    },
                    &self.header,
                );
                self.target.FillRectangle(
                    &D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: HEADER_WIDTH,
                        bottom: size.height as f32,
                    },
                    &self.header,
                );
                for visible_row in 0..rows {
                    let row = state.first_row + visible_row;
                    if row >= grid.row_count() {
                        break;
                    }
                    let y = HEADER_HEIGHT + visible_row as f32 * CELL_HEIGHT;
                    if row == state.selected_row {
                        self.target.FillRectangle(
                            &D2D_RECT_F {
                                left: 0.0,
                                top: y,
                                right: size.width as f32,
                                bottom: y + CELL_HEIGHT,
                            },
                            &self.selection,
                        );
                    }
                    draw_text(
                        &self.target,
                        &self.text_format,
                        &self.text,
                        &(row + 1).to_string(),
                        D2D_RECT_F {
                            left: 4.0,
                            top: y + 4.0,
                            right: HEADER_WIDTH - 3.0,
                            bottom: y + CELL_HEIGHT,
                        },
                    );
                    let values = grid.row(row).unwrap_or_default();
                    for visible_column in 0..columns {
                        let column = state.first_column + visible_column;
                        let x = HEADER_WIDTH + visible_column as f32 * CELL_WIDTH;
                        if row == state.selected_row && column == state.selected_column {
                            self.target.FillRectangle(
                                &D2D_RECT_F {
                                    left: x,
                                    top: y,
                                    right: x + CELL_WIDTH,
                                    bottom: y + CELL_HEIGHT,
                                },
                                &self.selection,
                            );
                        }
                        if let Some(value) = values.get(column) {
                            draw_text(
                                &self.target,
                                &self.text_format,
                                &self.text,
                                value,
                                D2D_RECT_F {
                                    left: x + 5.0,
                                    top: y + 4.0,
                                    right: x + CELL_WIDTH - 4.0,
                                    bottom: y + CELL_HEIGHT,
                                },
                            );
                        }
                    }
                }
                for visible_column in 0..columns {
                    let column = state.first_column + visible_column;
                    let x = HEADER_WIDTH + visible_column as f32 * CELL_WIDTH;
                    draw_text(
                        &self.target,
                        &self.text_format,
                        &self.text,
                        &column_name(column),
                        D2D_RECT_F {
                            left: x + 5.0,
                            top: 5.0,
                            right: x + CELL_WIDTH,
                            bottom: HEADER_HEIGHT,
                        },
                    );
                }
                for r in 0..=rows {
                    let y = HEADER_HEIGHT + r as f32 * CELL_HEIGHT;
                    self.target.DrawLine(
                        windows_numerics::Vector2 { X: 0.0, Y: y },
                        windows_numerics::Vector2 {
                            X: size.width as f32,
                            Y: y,
                        },
                        &self.line,
                        1.0,
                        None,
                    );
                }
                for c in 0..=columns {
                    let x = HEADER_WIDTH + c as f32 * CELL_WIDTH;
                    self.target.DrawLine(
                        windows_numerics::Vector2 { X: x, Y: 0.0 },
                        windows_numerics::Vector2 {
                            X: x,
                            Y: size.height as f32,
                        },
                        &self.line,
                        1.0,
                        None,
                    );
                }
            }
            let _ = self.target.EndDraw(None, None);
        }
    }
}

struct WebPreview {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

impl WebPreview {
    fn create(parent: HWND) -> std::result::Result<Self, webview2_com::Error> {
        let user_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("QuickPreview")
            .join("WebView2");
        std::fs::create_dir_all(&user_data)
            .map_err(|error| webview2_com::Error::CallbackError(error.to_string()))?;
        let user_data = user_data.to_string_lossy().into_owned();
        let environment = {
            let (tx, rx) = std::sync::mpsc::channel();
            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    let folder = CoTaskMemPWSTR::from(user_data.as_str());
                    let options: ICoreWebView2EnvironmentOptions =
                        CoreWebView2EnvironmentOptions::default().into();
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        *folder.as_ref().as_pcwstr(),
                        &options,
                        &handler,
                    )
                    .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |code, environment| {
                    code?;
                    tx.send(environment.ok_or_else(|| Error::from(E_POINTER)))
                        .map_err(|_| Error::from(E_POINTER))?;
                    Ok(())
                }),
            )?;
            webview2_com::wait_with_pump(rx).map_err(|_| webview2_com::Error::SendError)??
        };
        let controller = {
            let (tx, rx) = std::sync::mpsc::channel();
            CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    environment
                        .CreateCoreWebView2Controller(parent, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(move |code, controller| {
                    code?;
                    tx.send(controller.ok_or_else(|| Error::from(E_POINTER)))
                        .map_err(|_| Error::from(E_POINTER))?;
                    Ok(())
                }),
            )?;
            webview2_com::wait_with_pump(rx).map_err(|_| webview2_com::Error::SendError)??
        };
        let webview = unsafe {
            controller
                .CoreWebView2()
                .map_err(webview2_com::Error::WindowsError)?
        };
        unsafe {
            controller
                .SetBounds(client_rect(parent))
                .map_err(webview2_com::Error::WindowsError)?;
            controller
                .SetIsVisible(true)
                .map_err(webview2_com::Error::WindowsError)?;
            let settings = webview
                .Settings()
                .map_err(webview2_com::Error::WindowsError)?;
            settings
                .SetAreDevToolsEnabled(false)
                .map_err(webview2_com::Error::WindowsError)?;
            settings
                .SetAreDefaultContextMenusEnabled(false)
                .map_err(webview2_com::Error::WindowsError)?;
            settings
                .SetAreHostObjectsAllowed(false)
                .map_err(webview2_com::Error::WindowsError)?;
            settings
                .SetIsStatusBarEnabled(false)
                .map_err(webview2_com::Error::WindowsError)?;
            let mut navigation_token = 0;
            webview
                .add_NavigationStarting(
                    &NavigationStartingEventHandler::create(Box::new(|_sender, args| {
                        if let Some(args) = args {
                            let mut raw = PWSTR::null();
                            args.Uri(&mut raw)?;
                            let uri = if raw.is_null() {
                                String::new()
                            } else {
                                raw.to_string().unwrap_or_default()
                            };
                            if !raw.is_null() {
                                CoTaskMemFree(Some(raw.0 as *const c_void));
                            }
                            if uri != "about:blank" {
                                args.SetCancel(true)?;
                            }
                        }
                        Ok(())
                    })),
                    &mut navigation_token,
                )
                .map_err(webview2_com::Error::WindowsError)?;
            let mut token = 0;
            webview
                .add_WebMessageReceived(
                    &WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
                        if let Some(args) = args {
                            let mut source_raw = PWSTR::null();
                            args.Source(&mut source_raw)?;
                            let source = if source_raw.is_null() {
                                String::new()
                            } else {
                                source_raw.to_string().unwrap_or_default()
                            };
                            if !source_raw.is_null() {
                                CoTaskMemFree(Some(source_raw.0 as *const c_void));
                            }
                            if source != "about:blank" {
                                return Ok(());
                            }
                            let mut raw = PWSTR::null();
                            if args.WebMessageAsJson(&mut raw).is_ok() && !raw.is_null() {
                                let message = raw.to_string().unwrap_or_default();
                                CoTaskMemFree(Some(raw.0 as *const c_void));
                                let pointer = Box::into_raw(Box::new(message));
                                if PostMessageW(
                                    Some(parent),
                                    WM_WEB_MESSAGE,
                                    WPARAM(pointer as usize),
                                    LPARAM(0),
                                )
                                .is_err()
                                {
                                    drop(Box::from_raw(pointer));
                                }
                            }
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .map_err(webview2_com::Error::WindowsError)?;
        }
        Ok(Self {
            controller,
            webview,
        })
    }
    fn navigate_to_string(&self, html: &str) -> Result<()> {
        let wide = CoTaskMemPWSTR::from(html);
        unsafe { self.webview.NavigateToString(*wide.as_ref().as_pcwstr()) }
    }
    fn resize(&self, hwnd: HWND) {
        unsafe {
            let _ = self.controller.SetBounds(client_rect(hwnd));
        }
    }
}

impl Drop for WebPreview {
    fn drop(&mut self) {
        unsafe {
            let _ = self.controller.Close();
        }
    }
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let state = &mut *pointer;
        match message {
            WM_DOCUMENT_READY => {
                let result = Box::from_raw(wparam.0 as *mut quick_preview::Result<DocumentSession>);
                state.set_document(*result);
                LRESULT(0)
            }
            WM_WEB_MESSAGE => {
                let message = Box::from_raw(wparam.0 as *mut String);
                state.handle_web_message(*message);
                LRESULT(0)
            }
            WM_SIZE => {
                state.renderer.resize(hwnd);
                if let Some(webview) = &state.webview {
                    webview.resize(hwnd);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut paint);
                state.renderer.draw(state);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_DROPFILES => {
                let drop = HDROP(wparam.0 as *mut c_void);
                if let Some(path) = dropped_path(drop) {
                    begin_open(hwnd, path);
                }
                DragFinish(drop);
                LRESULT(0)
            }
            WM_COMMAND if ((wparam.0 >> 16) & 0xffff) as u32 == EN_KILLFOCUS => {
                state.commit_cell_edit();
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let x = (lparam.0 as i16) as i32;
                let y = ((lparam.0 >> 16) as i16) as i32;
                select_cell(state, x, y);
                state.begin_cell_edit();
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 as i16) as i32;
                let y = ((lparam.0 >> 16) as i16) as i32;
                select_cell(state, x, y);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) as i16) as i32;
                state.first_row = if delta > 0 {
                    state.first_row.saturating_sub(3)
                } else {
                    state.first_row.saturating_add(3)
                };
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                match wparam.0 as u16 {
                    key if ctrl && key == b'O' as u16 => {
                        if let Some(path) = open_dialog(hwnd) {
                            begin_open(hwnd, path);
                        }
                    }
                    key if ctrl && key == b'S' as u16 => state.save(),
                    key if ctrl && key == b'Z' as u16 => state.undo(false),
                    key if ctrl && key == b'Y' as u16 => state.undo(true),
                    key if key == VK_F2.0 => state.begin_cell_edit(),
                    key if key == VK_LEFT.0 => {
                        state.selected_column = state.selected_column.saturating_sub(1)
                    }
                    key if key == VK_RIGHT.0 => {
                        state.selected_column = state.selected_column.saturating_add(1)
                    }
                    key if key == VK_UP.0 => {
                        state.selected_row = state.selected_row.saturating_sub(1)
                    }
                    key if key == VK_DOWN.0 => {
                        state.selected_row = state.selected_row.saturating_add(1)
                    }
                    _ => return DefWindowProcW(hwnd, message, wparam, lparam),
                };
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_CLOSE => {
                if state.document.as_ref().is_some_and(|d| d.dirty)
                    && MessageBoxW(
                        Some(hwnd),
                        w!("未保存の変更があります。終了しますか？"),
                        w!("QuickPreview"),
                        MB_YESNO | MB_ICONWARNING,
                    ) != IDYES
                {
                    return LRESULT(0);
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(pointer));
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

unsafe extern "system" fn edit_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference: usize,
) -> LRESULT {
    if message == WM_KEYDOWN && wparam.0 as u16 == VK_RETURN.0 {
        if let Ok(parent) = unsafe { GetParent(hwnd) } {
            unsafe {
                let _ = SetFocus(Some(parent));
            }
        }
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn begin_open(hwnd: HWND, path: PathBuf) {
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
        if !pointer.is_null() {
            if (*pointer).loading {
                return;
            }
            if (*pointer)
                .document
                .as_ref()
                .is_some_and(|document| document.dirty)
                && MessageBoxW(
                    Some(hwnd),
                    w!("未保存の変更を破棄して別のファイルを開きますか？"),
                    w!("QuickPreview"),
                    MB_YESNO | MB_ICONWARNING,
                ) != IDYES
            {
                return;
            }
            (*pointer).loading = true;
            (*pointer).webview = None;
        }
    }
    let raw_hwnd = hwnd.0 as usize;
    thread::spawn(move || {
        let result = DocumentSession::open(path);
        let pointer = Box::into_raw(Box::new(result));
        unsafe {
            let target = HWND(raw_hwnd as *mut c_void);
            if PostMessageW(
                Some(target),
                WM_DOCUMENT_READY,
                WPARAM(pointer as usize),
                LPARAM(0),
            )
            .is_err()
            {
                drop(Box::from_raw(pointer));
            }
        }
    });
}

fn select_cell(state: &mut AppState, x: i32, y: i32) {
    if x >= HEADER_WIDTH as i32 && y >= HEADER_HEIGHT as i32 {
        state.selected_column =
            state.first_column + ((x as f32 - HEADER_WIDTH) / CELL_WIDTH) as usize;
        state.selected_row = state.first_row + ((y as f32 - HEADER_HEIGHT) / CELL_HEIGHT) as usize;
        unsafe {
            let _ = InvalidateRect(Some(state.hwnd), None, false);
        }
    }
}

fn open_dialog(hwnd: HWND) -> Option<PathBuf> {
    let mut buffer = [0u16; 32_768];
    let filter: Vec<u16> =
        "Supported files\0*.csv;*.tsv;*.md;*.markdown;*.html;*.htm\0All files\0*.*\0\0"
            .encode_utf16()
            .collect();
    let mut dialog = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        let length = buffer.iter().position(|c| *c == 0).unwrap_or(0);
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
    } else {
        None
    }
}

fn dropped_path(drop: HDROP) -> Option<PathBuf> {
    let length = unsafe { DragQueryFileW(drop, 0, None) };
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe {
        DragQueryFileW(drop, 0, Some(&mut buffer));
    }
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}
fn client_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    rect
}
fn client_size(hwnd: HWND) -> D2D_SIZE_U {
    let rect = client_rect(hwnd);
    D2D_SIZE_U {
        width: (rect.right - rect.left).max(1) as u32,
        height: (rect.bottom - rect.top).max(1) as u32,
    }
}
fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn window_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let mut text = vec![0u16; length as usize + 1];
    unsafe {
        GetWindowTextW(hwnd, &mut text);
    }
    String::from_utf16_lossy(&text[..length as usize])
}
fn show_error(hwnd: HWND, message: &str) {
    let message = wide(message);
    unsafe {
        MessageBoxW(
            Some(hwnd),
            PCWSTR(message.as_ptr()),
            w!("QuickPreview"),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn column_name(mut column: usize) -> String {
    let mut result = String::new();
    loop {
        result.insert(0, (b'A' + (column % 26) as u8) as char);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    result
}
fn draw_text(
    target: &ID2D1HwndRenderTarget,
    format: &IDWriteTextFormat,
    brush: &ID2D1SolidColorBrush,
    value: &str,
    rect: D2D_RECT_F,
) {
    let utf16: Vec<u16> = value.encode_utf16().collect();
    unsafe {
        target.DrawText(
            &utf16,
            format,
            &rect,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}
