use ::crossterm::event::*;
use ::crossterm::terminal::ClearType;
use ::crossterm::style::*;
use ::crossterm::{cursor, event, execute, queue, style, terminal};
use std::io::{self, stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{cmp, env, fs};

// PROMPT MACRO TEXTUAL DEFINITION
#[macro_export]
macro_rules! prompt {
    ($output:expr,$args:tt) => {
        prompt!($output, $args, callback = |&_, _, _| {})
    };

    ($output:expr,$args:tt, callback = $callback:expr) => {{
        let output: &mut Output = $output;
        let mut input = String::with_capacity(32);

        loop {
            output.status_message.set_message(format!($args, input));
            output.refresh_screen()?;

            let key_event = Reader.read_key()?;

            match key_event {
                KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                } => {
                    if !input.is_empty() {
                        output.status_message.set_message(String::new());
                        $callback(output, &input, KeyCode::Enter);
                        break;
                    }
                }
                KeyEvent {
                    code: KeyCode::Esc,
                    ..
                } => {
                    output.status_message.set_message(String::new());
                    input.clear();
                    $callback(output, &input, KeyCode::Esc);
                    break;
                }
                KeyEvent {
                    code: KeyCode::Backspace | KeyCode::Delete,
                    modifiers: KeyModifiers::NONE,
                } => {
                    input.pop();
                }
                KeyEvent {
                    code: code @ (KeyCode::Char(..) | KeyCode::Tab),
                    modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                } => input.push(match code {
                    KeyCode::Tab => '\t',
                    KeyCode::Char(ch) => ch,
                    _ => unreachable!(),
                }),
                _ => {}
            }

            $callback(output, &input, key_event.code);
        }

        if input.is_empty() {None} else {Some(input)}
    }};
}

// Cleanup struct is used to disable raw mode
// Called at the start of main() and when it goes
// out of scope, the drop() implmentation is called
struct Cleanup;

const VERSION: f32 = 1.0;

impl Drop for Cleanup {
    fn drop(&mut self) {
        terminal::disable_raw_mode().expect("Couldn't disable raw mode");
        Output::clear_screen().expect("Error clearing screen");
    }
}

// Used to move around the cursor based on
// some user key presses
#[derive(Clone, Copy)]
struct CursorController {
    cursor_x: usize,
    cursor_y: usize,
    screen_columns: usize,
    screen_rows: usize,
    row_offset: usize,
    column_offset: usize,
    render_x: usize,
}

impl CursorController {
    fn new(win_size: (usize, usize)) -> CursorController {
        Self {
            cursor_x: 0,
            cursor_y: 0,
            screen_columns: win_size.0,
            screen_rows: win_size.1,
            row_offset: 0,
            column_offset: 0,
            render_x: 0,
        }
    }

    fn get_render_x(&self, row: &Row) -> usize {
        row.row_content[..self.cursor_x]
            .chars()
            .fold(0, |render_x, c| {
                if c == '\t' {
                    render_x + (TAB_STOP - 1) - (render_x % TAB_STOP) + 1
                } else {
                    render_x + 1
                }
            })
    }

    fn move_cursor(&mut self, direction: KeyCode, editor_rows: &EditorRows) {
        let number_of_rows = editor_rows.number_of_rows();

        match direction {
            KeyCode::Up => {
                self.cursor_y = self.cursor_y.saturating_sub(1);
            }
            KeyCode::Left => {
                if self.cursor_x != 0 {
                    self.cursor_x -= 1;
                } else if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                    self.cursor_x = editor_rows.get_row(self.cursor_y).len();
                }
            }
            KeyCode::Down => {
                if self.cursor_y < number_of_rows {
                    self.cursor_y += 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_y < number_of_rows {
                    if self.cursor_x < editor_rows.get_row(self.cursor_y).len() {
                        self.cursor_x += 1;
                    } else {
                        self.cursor_x = 0;
                        self.cursor_y += 1;
                    }
                }
            }
            KeyCode::Home => self.cursor_x = 0,
            KeyCode::End => {
                if self.cursor_y < number_of_rows {
                    self.cursor_x = editor_rows.get_row(self.cursor_y).len();
                }
            }
            _ => unimplemented!(),
        }

        let row_len = if self.cursor_y < number_of_rows {
            editor_rows.get_row(self.cursor_y).len()
        } else {
            0
        };

        self.cursor_x = cmp::min(self.cursor_x, row_len);
    }

    fn scroll(&mut self, editor_rows: &EditorRows) {
        self.render_x = 0;
        if self.cursor_y < editor_rows.number_of_rows() {
            self.render_x = self.get_render_x(editor_rows.get_editor_row(self.cursor_y))
        }

        // vertical scroll
        self.row_offset = cmp::min(self.row_offset, self.cursor_y);
        if self.cursor_y >= self.row_offset + self.screen_rows {
            self.row_offset = self.cursor_y - self.screen_rows + 1;
        }

        // horizontal scroll
        self.column_offset = cmp::min(self.column_offset, self.render_x);
        if self.render_x >= self.column_offset + self.screen_columns {
            self.column_offset = self.render_x - self.screen_columns + 1;
        }
    }
}

struct StatusMessage {
    message: Option<String>,
    set_time: Option<Instant>,
}

impl StatusMessage {
    fn new(initial_message: String) -> Self {
        Self {
            message: Some(initial_message),
            set_time: Some(Instant::now()),
        }
    }

    fn set_message(&mut self, message: String) {
        self.message = Some(message);
        self.set_time = Some(Instant::now())
    }

    fn message(&mut self) -> Option<&String> {
        self.set_time.and_then(|time| {
            if time.elapsed() > Duration::from_secs(5) {
                self.message = None;
                self.set_time = None;
                None
            } else {
                Some(self.message.as_ref().unwrap())
            }
        })
    }
}

enum SearchDirection {
    Forward,
    Backward,
}

struct SearchIndex {
    x_index: usize,
    y_index: usize,
    x_direction: Option<SearchDirection>,
    y_direction: Option<SearchDirection>,
}

impl SearchIndex {
    fn new() -> Self {
        Self {
            x_index: 0,
            y_index: 0,
            x_direction: None,
            y_direction: None,
        }
    }

    fn reset(&mut self) {
        self.x_index = 0;
        self.y_index = 0;
        self.x_direction = None;
        self.y_direction = None;
    }
}

// Output struct is used to handle the output to the
// terminal screen. This includes the ~ at the start of
// each line like Vim and also used to ensure that
// instead of multiple small writes happening each time,
// one big write happens (efficient)
struct Output {
    win_size: (usize, usize),
    editor_contents: EditorContents,
    cursor_controller: CursorController,
    editor_rows: EditorRows,
    status_message: StatusMessage,
    dirty: u64,
    search_index: SearchIndex,
}

impl Output {
    fn new() -> Self {
        // Get window size of the current terminal screen
        // and instantiate the output with the window size
        // and the contents that will go there
        let win_size = terminal::size()
            .map(|(x, y)| (x as usize, y as usize - 2))
            .unwrap();
        Self {
            win_size,
            editor_contents: EditorContents::new(),
            cursor_controller: CursorController::new(win_size),
            editor_rows: EditorRows::new(),
            status_message: StatusMessage::new("Help: CTRL + S to Save | CTRL + F to Find | CTRL + Q to Quit.".into()),
            dirty: 0,
            search_index: SearchIndex::new(),
        }
    }

    fn clear_screen() -> crossterm::Result<()> {
        // Associate function that will be called whenever
        // there is a need to clear screen and relocate the cursor
        // to the top left
        execute!(stdout(), terminal::Clear(ClearType::All))?;
        execute!(stdout(), cursor::MoveTo(0, 0))
    }

    fn insert_char(&mut self, ch: char) {
        if self.cursor_controller.cursor_y == self.editor_rows.number_of_rows() {
            self.editor_rows
                .insert_row(self.editor_rows.number_of_rows(), String::new());

            self.dirty += 1;
        }

        self.editor_rows
            .get_editor_row_mut(self.cursor_controller.cursor_y)
            .insert_char(self.cursor_controller.cursor_x, ch);

        self.cursor_controller.cursor_x += 1;

        // tracks that file has been modified
        // counts the amount of changes
        self.dirty += 1;
    }

    fn insert_newline(&mut self) {
        if self.cursor_controller.cursor_x == 0 {
            self.editor_rows
                .insert_row(self.cursor_controller.cursor_y, String::new())
        } else {
            let current_row = self
                .editor_rows
                .get_editor_row_mut(self.cursor_controller.cursor_y);

            let new_row_content: String =
                current_row.row_content[self.cursor_controller.cursor_x..].into();

            current_row
                .row_content
                .truncate(self.cursor_controller.cursor_x);

            EditorRows::render_row(current_row);
            self.editor_rows
                .insert_row(self.cursor_controller.cursor_y + 1, new_row_content);
        }

        self.cursor_controller.cursor_x = 0;
        self.cursor_controller.cursor_y += 1;

        self.dirty += 1;
    }

    fn delete_char(&mut self) {
        if self.cursor_controller.cursor_y == self.editor_rows.number_of_rows() {
            return;
        }

        let row = self
            .editor_rows
            .get_editor_row_mut(self.cursor_controller.cursor_y);

        if self.cursor_controller.cursor_x > 0 {
            row.delete_char(self.cursor_controller.cursor_x - 1);
            self.cursor_controller.cursor_x -= 1;
        } else {
            let previous_row_content = self
                .editor_rows
                .get_row(self.cursor_controller.cursor_y - 1);

            self.cursor_controller.cursor_x = previous_row_content.len();

            self.editor_rows
                .join_adjacent_rows(self.cursor_controller.cursor_y);

            self.cursor_controller.cursor_y -= 1;
        }
        self.dirty += 1;
    }

    fn find_callback(output: &mut Output, keyword: &str, key_code: KeyCode) {
        match key_code {
            KeyCode::Esc | KeyCode::Enter => {
                output.search_index.reset();
            }
            _ => {
                output.search_index.y_direction = None;
                output.search_index.x_direction = None;

                match key_code {
                    KeyCode::Down => {
                        output.search_index.y_direction = SearchDirection::Forward.into()
                    }
                    KeyCode::Up => {
                        output.search_index.y_direction = SearchDirection::Backward.into()
                    }
                    KeyCode::Left => {
                        output.search_index.x_direction = SearchDirection::Backward.into()
                    }
                    KeyCode::Right => {
                        output.search_index.x_direction = SearchDirection::Forward.into()
                    }
                    _ => {}
                }

                for i in 0..output.editor_rows.number_of_rows() {
                    let row_index = match output.search_index.y_direction.as_ref() {
                        None => {
                            if output.search_index.x_direction.is_none() {
                                output.search_index.y_index = i;
                            }

                            output.search_index.y_index
                        }
                        Some(dir) => {
                            if matches!(dir, SearchDirection::Forward) {
                                output.search_index.y_index + i + 1
                            } else {
                                let res = output.search_index.y_index.saturating_sub(i);
                                if res == 0 {
                                    break;
                                }
                                res - 1
                            }
                        }
                    };

                    if row_index > output.editor_rows.number_of_rows() - 1 {
                        break;
                    }

                    let row = output.editor_rows.get_editor_row(row_index);
                    let index = match output.search_index.x_direction.as_ref() {
                        None => row.render.find(&keyword),
                        Some(dir) => {
                            let index = if matches!(dir, SearchDirection::Forward) {
                                let start = cmp::min(row.render.len(),
                                    output.search_index.x_index + 1);

                                row.render[start..]
                                    .find(&keyword)
                                    .map(|index| index + start)
                            } else{
                                row.render[..output.search_index.x_index].rfind(&keyword)
                            };

                            if index.is_none() {
                                break;
                            }

                            index
                        }
                    };

                    if let Some(index) = index {
                        output.cursor_controller.cursor_y = row_index;
                        output.search_index.y_index = row_index;
                        output.search_index.x_index = index;

                        output.cursor_controller.cursor_x = row.get_row_content_x(index);
                        output.cursor_controller.row_offset = output.editor_rows.number_of_rows();
                        break;
                    }
                }
            }
        }
    }

    fn find(&mut self) -> io::Result<()> {
        let cursor_controller = self.cursor_controller;
        
        if prompt!(
            self,
            "Search: {} (ESC to cancel, Arrows to find next matches, Enter to find)",
            callback = Output::find_callback
        ).is_none() {
            self.cursor_controller = cursor_controller;
        }
        Ok(())
    }

    fn draw_rows(&mut self) {
        // Draws each row in the terminal window based on the size
        // saved when initialized. Includes drawing the ~ at the start
        // of each row and also a welcome message at the horizontal center
        // of the screen, a third of the way down vertically.
        let screen_rows = self.win_size.1;
        let screen_columns = self.win_size.0;

        for i in 0..screen_rows {
            let file_row = i + self.cursor_controller.row_offset;

            if file_row >= self.editor_rows.number_of_rows() {
                if self.editor_rows.number_of_rows() == 0 && i == screen_rows / 3 {
                    let mut welcome = format!("Pound editor --- Version {}", VERSION);

                    if welcome.len() > screen_columns {
                        welcome.truncate(screen_columns)
                    }

                    let mut padding = (screen_columns - welcome.len()) / 2;
                    if padding != 0 {
                        self.editor_contents.push('~');
                        padding -= 1;
                    }
                    (0..padding).for_each(|_| self.editor_contents.push(' '));

                    self.editor_contents.push_str(&welcome);
                } else {
                    self.editor_contents.push('~');
                }
            } else {
                let row = self.editor_rows.get_render(file_row);
                let column_offset = self.cursor_controller.column_offset;

                let len = if row.len() < column_offset {
                    0
                } else {
                    let len = row.len() - column_offset;
                    if len > screen_columns {
                        screen_columns
                    } else {
                        len
                    }
                };

                let start = if len == 0 { 0 } else { column_offset };

                row[start..start + len].chars().for_each(|c| {
                    if c.is_digit(10) {
                        let _ = queue!(self.editor_contents, SetForegroundColor(Color::Cyan));
                        self.editor_contents.push(c);
                        let _ = queue!(self.editor_contents, ResetColor);
                    } else {
                        self.editor_contents.push(c);
                    }
                });

                // self.editor_contents.push_str(&row[start..start + len]);
            }
            queue!(
                self.editor_contents,
                terminal::Clear(ClearType::UntilNewLine)
            )
            .unwrap();

            self.editor_contents.push_str("\r\n");
        }
    }

    fn draw_status_bar(&mut self) {
        self.editor_contents
            .push_str(&style::Attribute::Reverse.to_string());

        let info = format!(
            "{} {} -- {} lines",
            self.editor_rows
                .filename
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("[No Name]"),
            if self.dirty > 0 { "(modified)" } else { "" },
            self.editor_rows.number_of_rows()
        );

        let info_len = cmp::min(info.len(), self.win_size.0);

        let line_info = format!(
            "{}/{}",
            self.cursor_controller.cursor_y + 1,
            self.editor_rows.number_of_rows()
        );

        self.editor_contents.push_str(&info[..info_len]);
        for i in info_len..self.win_size.0 {
            if self.win_size.0 - i == line_info.len() {
                self.editor_contents.push_str(&line_info);
                break;
            } else {
                self.editor_contents.push(' ')
            }
        }

        self.editor_contents.push_str("\r\n");
        self.editor_contents
            .push_str(&style::Attribute::Reset.to_string());
    }

    fn draw_message_bar(&mut self) {
        // Draws out any message passed in at the very bottom
        // of the screen
        queue!(
            self.editor_contents,
            terminal::Clear(ClearType::UntilNewLine)
        )
        .unwrap();

        if let Some(msg) = self.status_message.message() {
            self.editor_contents
                .push_str(&msg[..cmp::min(self.win_size.0, msg.len())]);
        }
    }

    fn refresh_screen(&mut self) -> crossterm::Result<()> {
        // 'queue' will queue commands to be run in the terminal
        // (provided by crossterm)
        // Hide the cursor before updates and relocate it to the top left
        // Show it back when update finishes
        // Also calls scroll
        self.cursor_controller.scroll(&self.editor_rows);
        queue!(self.editor_contents, cursor::Hide, cursor::MoveTo(0, 0))?;

        self.draw_rows();
        self.draw_status_bar();
        self.draw_message_bar();

        // Move the cursor to particular location based on
        // the cursor controller class
        let cursor_x = self.cursor_controller.render_x - self.cursor_controller.column_offset;
        let cursor_y = self.cursor_controller.cursor_y - self.cursor_controller.row_offset;

        queue!(
            self.editor_contents,
            cursor::MoveTo(cursor_x as u16, cursor_y as u16),
            cursor::Show,
        )?;
        self.editor_contents.flush()
    }

    fn move_cursor(&mut self, direction: KeyCode) {
        self.cursor_controller
            .move_cursor(direction, &self.editor_rows);
    }
}

// Reader struct is used to read keypresses by the user
struct Reader;

impl Reader {
    // Read the key pressed by the user and check every
    // 5 seconds for input
    fn read_key(&self) -> crossterm::Result<KeyEvent> {
        loop {
            if event::poll(Duration::from_millis(5000))? {
                if let Event::Key(event) = event::read()? {
                    return Ok(event);
                }
            }
        }
    }
}

// Used to store row content and row render
// content
#[derive(Default)]
struct Row {
    row_content: String,
    render: String,
}

impl Row {
    fn new(row_content: String, render: String) -> Self {
        Self {
            row_content,
            render,
        }
    }

    fn insert_char(&mut self, at: usize, ch: char) {
        self.row_content.insert(at, ch);
        EditorRows::render_row(self);
    }

    fn delete_char(&mut self, at: usize) {
        self.row_content.remove(at);
        EditorRows::render_row(self);
    }

    fn get_row_content_x(&self, render_x: usize) -> usize {
        let mut current_render_x = 0;

        for (cursor_x, ch) in self.row_content.chars().enumerate() {
            if ch == '\t' {
                current_render_x += (TAB_STOP - 1) - (current_render_x % TAB_STOP);
            }

            current_render_x += 1;

            if current_render_x > render_x {
                return cursor_x;
            }
        }

        0
    }
}

const TAB_STOP: usize = 8;
// Used to store contents of rows in the
struct EditorRows {
    row_contents: Vec<Row>,
    filename: Option<PathBuf>,
}

impl EditorRows {
    fn new() -> Self {
        let mut arg = env::args();

        match arg.nth(1) {
            None => Self {
                row_contents: Vec::new(),
                filename: None,
            },
            Some(file) => Self::from_file(file.into()),
        }
    }

    fn render_row(row: &mut Row) {
        let mut index = 0;

        let capacity = row
            .row_content
            .chars()
            .fold(0, |acc, next| acc + if next == '\t' { TAB_STOP } else { 1 });

        row.render = String::with_capacity(capacity);
        row.row_content.chars().for_each(|c| {
            index += 1;
            if c == '\t' {
                row.render.push(' ');
                while index % TAB_STOP != 0 {
                    row.render.push(' ');
                    index += 1;
                }
            } else {
                row.render.push(c);
            }
        });
    }

    fn insert_row(&mut self, at: usize, contents: String) {
        // self.row_contents.push(Row::default());
        let mut new_row = Row::new(contents, String::new());
        EditorRows::render_row(&mut new_row);
        self.row_contents.insert(at, new_row);
    }

    fn join_adjacent_rows(&mut self, at: usize) {
        let current_row = self.row_contents.remove(at);
        let previous_row = self.get_editor_row_mut(at - 1);

        previous_row.row_content.push_str(&current_row.row_content);
        Self::render_row(previous_row);
    }

    fn get_editor_row_mut(&mut self, at: usize) -> &mut Row {
        &mut self.row_contents[at]
    }

    fn from_file(file: PathBuf) -> Self {
        let file_contents = fs::read_to_string(&file).expect("Unable to read file");

        Self {
            filename: Some(file),
            row_contents: file_contents
                .lines()
                .map(|it| {
                    let mut row = Row::new(it.into(), String::new());
                    Self::render_row(&mut row);
                    row
                })
                .collect(),
        }
    }

    fn get_render(&self, at: usize) -> &String {
        &self.row_contents[at].render
    }

    fn get_editor_row(&self, at: usize) -> &Row {
        &self.row_contents[at]
    }

    fn number_of_rows(&self) -> usize {
        self.row_contents.len()
    }

    fn get_row(&self, at: usize) -> &str {
        &self.row_contents[at].row_content
    }

    fn save(&self) -> io::Result<usize> {
        match &self.filename {
            None => Err(io::Error::new(
                io::ErrorKind::Other,
                "no file name specified",
            )),
            Some(name) => {
                let mut file = fs::OpenOptions::new().write(true).create(true).open(name)?;
                let contents: String = self
                    .row_contents
                    .iter()
                    .map(|it| it.row_content.as_str())
                    .collect::<Vec<&str>>()
                    .join("\n");
                file.set_len(contents.len() as u64)?;
                file.write_all(contents.as_bytes())?;
                Ok(contents.as_bytes().len())
            }
        }
    }
}

// The actual text editor struct, includes the key
// press reader and also the output that will be displayed
// in this text editor.
struct Editor {
    reader: Reader,
    output: Output,
    quit_times: u8,
}

const QUIT_TIMES: u8 = 2;

impl Editor {
    fn new() -> Self {
        Self {
            reader: Reader,
            output: Output::new(),
            quit_times: QUIT_TIMES,
        }
    }

    fn process_keypress(&mut self) -> crossterm::Result<bool> {
        // Check what key is pressed by the user
        // quit editor if Ctrl+q is pressed
        // Ctrl, Shift etc are called Key Modifiers
        match self.reader.read_key()? {
            KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: event::KeyModifiers::CONTROL,
            } => {
                if self.output.dirty > 0 && self.quit_times > 0 {
                    self.output.status_message.set_message(format!(
                        "WARNING! File has unsaved changes. Press Ctrl+q {} more times to quit.",
                        self.quit_times
                    ));
                    // decrement quit times each time Ctrl+q is pressed
                    self.quit_times -= 1;
                    return Ok(true);
                }

                return Ok(false);
            }
            KeyEvent {
                code:
                    direction @ (KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End),
                modifiers: event::KeyModifiers::NONE,
            } => self.output.move_cursor(direction),
            KeyEvent {
                // Used to move to top and bottom of page instantly
                code: val @ (KeyCode::PageUp | KeyCode::PageDown),
                modifiers: event::KeyModifiers::NONE,
            } => {
                if matches!(val, KeyCode::PageUp) {
                    self.output.cursor_controller.cursor_y =
                        self.output.cursor_controller.row_offset
                } else {
                    self.output.cursor_controller.cursor_y = cmp::min(
                        self.output.win_size.1 + self.output.cursor_controller.row_offset - 1,
                        self.output.editor_rows.number_of_rows(),
                    );
                }
            }
            KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::CONTROL,
            } => {
                if matches!(self.output.editor_rows.filename, None) {
                    let prompt = prompt!(&mut self.output, "Save as: {} (ESC to cancel)").map(|it| it.into());

                    if let None = prompt {
                        self.output
                            .status_message
                            .set_message("Save aborted".into());
                        return Ok(true);
                    }

                    self.output.editor_rows.filename = prompt;
                }

                self.output.editor_rows.save().map(|len| {
                    self.output
                        .status_message
                        .set_message(format!("{} bytes written to disk", len));
                    self.output.dirty = 0;
                })?;
            }
            KeyEvent {
                code: KeyCode::Char('g'),
                modifiers: KeyModifiers::CONTROL,
            } => {
                self.output.find()?;
            }
            KeyEvent {
                code: key @ (KeyCode::Backspace | KeyCode::Delete),
                modifiers: KeyModifiers::NONE,
            } => {
                // Delete means delete char to the right
                // this is done by moving the cursor to the right
                // one step.

                // Then regardless of whether Backspace or Delete
                // is pressed, the appropriate function occurs
                if matches!(key, KeyCode::Delete) {
                    self.output.move_cursor(KeyCode::Right)
                }

                self.output.delete_char();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
            } => self.output.insert_newline(),
            KeyEvent {
                // Used to handle a user input to the text 'editor'
                // Handles any other key pressed by the user
                // That isn't already mapped above.
                // Also prevents modifiers like Ctrl to be used to enter
                // characters (Ex: Ctrl + X shouldn't insert X).
                code: code @ (KeyCode::Char(..) | KeyCode::Tab),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            } => self.output.insert_char(match code {
                KeyCode::Tab => '\t',
                KeyCode::Char(ch) => ch,
                _ => unreachable!(),
            }),
            _ => {}
        }

        Ok(true)
    }

    fn run(&mut self) -> crossterm::Result<bool> {
        // When editor is run, refresh the screen first
        // then start processing key presses by user
        self.output.refresh_screen()?;
        self.process_keypress()
    }
}

// Used to store contents of editor for one big write
// instead of many smaller writes
struct EditorContents {
    content: String,
}

impl EditorContents {
    fn new() -> Self {
        Self {
            content: String::new(),
        }
    }

    fn push(&mut self, ch: char) {
        self.content.push(ch)
    }

    fn push_str(&mut self, string: &str) {
        self.content.push_str(string)
    }
}

impl io::Write for EditorContents {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match std::str::from_utf8(buf) {
            Ok(s) => {
                self.content.push_str(s);
                Ok(s.len())
            }
            Err(_) => Err(io::ErrorKind::WriteZero.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let out = write!(stdout(), "{}", self.content);
        stdout().flush()?;
        self.content.clear();
        out
    }
}

fn main() -> crossterm::Result<()> {
    let _clean_up = Cleanup;
    terminal::enable_raw_mode()?;

    let mut editor = Editor::new();
    while editor.run()? {}
    Ok(())
}
