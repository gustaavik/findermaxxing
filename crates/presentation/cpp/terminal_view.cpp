#include "presentation/cpp/terminal_view.h"

#include <QtGui/QFontDatabase>
#include <QtGui/QFontMetricsF>
#include <QtGui/QInputMethodEvent>
#include <QtGui/QKeyEvent>
#include <QtGui/QPainter>
#include <QtGui/QWheelEvent>

#include <algorithm>
#include <cmath>

namespace {

// Must match `TextStyle` in application::ports::terminal.
constexpr std::uint8_t kBold = 1;
constexpr std::uint8_t kItalic = 1 << 1;
constexpr std::uint8_t kUnderline = 1 << 2;
constexpr std::uint8_t kDim = 1 << 3;
constexpr std::uint8_t kStrikeout = 1 << 4;
// Cursor shapes, as `TerminalCursor::shape`.
constexpr std::uint8_t kCursorBlock = 0;
constexpr std::uint8_t kCursorUnderline = 1;
constexpr std::uint8_t kCursorBar = 2;
constexpr qreal kCursorLineWidth = 2;
constexpr qreal kDimOpacity = 0.6;
// One notch of a mouse wheel.
constexpr qreal kWheelNotch = 120;
constexpr int kLinesPerNotch = 3;
constexpr qreal kPixelsPerLineFallback = 16;

QColor fromArgb(std::uint32_t argb)
{
    return QColor::fromRgba(argb);
}

} // namespace

TerminalView::TerminalView(QQuickItem* parent)
    : QQuickPaintedItem(parent)
    , m_font(QFontDatabase::systemFont(QFontDatabase::FixedFont))
{
    setFlag(ItemAcceptsInputMethod, true);
    setFlag(ItemIsFocusScope, false);
    setAcceptedMouseButtons(Qt::LeftButton);
    setActiveFocusOnTab(false);
    setOpaquePainting(true);
    const QFontMetricsF metrics(m_font);
    m_cellWidth = metrics.horizontalAdvance(QLatin1Char('M'));
    m_cellHeight = std::ceil(metrics.height());
    m_ascent = metrics.ascent();
}

int TerminalView::columns() const
{
    return m_columns;
}

int TerminalView::rows() const
{
    return m_rows;
}

void TerminalView::setFrame(rust::Vec<TerminalRun> runs, TerminalCursor cursor, std::uint32_t background)
{
    m_runs.clear();
    m_runs.reserve(runs.size());
    for (const TerminalRun& run : runs) {
        m_runs.push_back(Run {
            run.row,
            run.column,
            run.cells,
            QString::fromUtf8(run.text.data(), static_cast<qsizetype>(run.text.size())),
            fromArgb(run.foreground),
            fromArgb(run.background),
            run.style,
        });
    }
    m_cursor = cursor;
    m_background = fromArgb(background);
    update();
}

void TerminalView::paint(QPainter* painter)
{
    painter->fillRect(boundingRect(), m_background);
    for (const Run& run : m_runs) {
        const QRectF cells(run.column * m_cellWidth, run.row * m_cellHeight, run.cells * m_cellWidth, m_cellHeight);
        if (run.background != m_background) {
            painter->fillRect(cells, run.background);
        }
        QFont font = m_font;
        font.setBold(run.style & kBold);
        font.setItalic(run.style & kItalic);
        font.setUnderline(run.style & kUnderline);
        font.setStrikeOut(run.style & kStrikeout);
        painter->setFont(font);
        QColor foreground = run.foreground;
        if (run.style & kDim) {
            foreground.setAlphaF(kDimOpacity);
        }
        painter->setPen(foreground);
        // Each character at its own cell, so wide glyphs and fallback fonts
        // can't push the rest of the row out of line.
        qreal x = cells.x();
        const qreal baseline = cells.y() + m_ascent;
        for (const QChar character : run.text) {
            if (character.isLowSurrogate()) {
                continue;
            }
            painter->drawText(QPointF(x, baseline), QString(character));
            x += m_cellWidth;
        }
    }
    if (m_cursor.shape > kCursorBar) {
        return;
    }
    const QColor cursorColor = fromArgb(m_cursor.color);
    const QRectF cell(m_cursor.column * m_cellWidth, m_cursor.row * m_cellHeight, m_cellWidth, m_cellHeight);
    if (!hasActiveFocus()) {
        painter->setPen(QPen(cursorColor, 1));
        painter->setBrush(Qt::NoBrush);
        painter->drawRect(cell.adjusted(0.5, 0.5, -0.5, -0.5));
    } else if (m_cursor.shape == kCursorBlock) {
        QColor block = cursorColor;
        block.setAlphaF(kDimOpacity);
        painter->fillRect(cell, block);
    } else if (m_cursor.shape == kCursorUnderline) {
        painter->fillRect(QRectF(cell.left(), cell.bottom() - kCursorLineWidth, cell.width(), kCursorLineWidth), cursorColor);
    } else {
        painter->fillRect(QRectF(cell.left(), cell.top(), kCursorLineWidth, cell.height()), cursorColor);
    }
}

bool TerminalView::isShortcut(QKeyEvent* event, bool copy) const
{
#ifdef Q_OS_MACOS
    // Command-C/V; Control-C must still reach the shell.
    return event->matches(copy ? QKeySequence::Copy : QKeySequence::Paste);
#else
    // Ctrl+Shift+C/V, as in GNOME Terminal and Konsole.
    const Qt::KeyboardModifiers wanted = Qt::ControlModifier | Qt::ShiftModifier;
    return (event->modifiers() & wanted) == wanted && event->key() == (copy ? Qt::Key_C : Qt::Key_V);
#endif
}

void TerminalView::keyPressEvent(QKeyEvent* event)
{
    if (isShortcut(event, true)) {
        emit copyRequested();
    } else if (isShortcut(event, false)) {
        emit pasteRequested();
    } else {
        emit keyTyped(event->key(), static_cast<int>(event->modifiers()), event->text());
    }
    event->accept();
}

void TerminalView::inputMethodEvent(QInputMethodEvent* event)
{
    if (!event->commitString().isEmpty()) {
        emit textTyped(event->commitString());
    }
    event->accept();
}

QVariant TerminalView::inputMethodQuery(Qt::InputMethodQuery query) const
{
    if (query == Qt::ImCursorRectangle) {
        return QRectF(m_cursor.column * m_cellWidth, m_cursor.row * m_cellHeight, m_cellWidth, m_cellHeight);
    }
    if (query == Qt::ImEnabled) {
        return true;
    }
    return QQuickPaintedItem::inputMethodQuery(query);
}

QPoint TerminalView::cellAt(QPointF position) const
{
    const int column = std::clamp(static_cast<int>(position.x() / m_cellWidth), 0, std::max(m_columns - 1, 0));
    const int row = std::clamp(static_cast<int>(position.y() / m_cellHeight), 0, std::max(m_rows - 1, 0));
    return QPoint(column, row);
}

void TerminalView::mousePressEvent(QMouseEvent* event)
{
    forceActiveFocus(Qt::MouseFocusReason);
    m_selectionStart = cellAt(event->position());
    m_selecting = false;
    emit selectionCleared();
    event->accept();
}

void TerminalView::mouseMoveEvent(QMouseEvent* event)
{
    const QPoint here = cellAt(event->position());
    if (here != m_selectionStart || m_selecting) {
        m_selecting = true;
        emit selecting(m_selectionStart.y(), m_selectionStart.x(), here.y(), here.x());
    }
    event->accept();
}

void TerminalView::mouseReleaseEvent(QMouseEvent* event)
{
    event->accept();
}

void TerminalView::wheelEvent(QWheelEvent* event)
{
    qreal lines = 0;
    if (!event->pixelDelta().isNull()) {
        const qreal lineHeight = m_cellHeight > 0 ? m_cellHeight : kPixelsPerLineFallback;
        lines = event->pixelDelta().y() / lineHeight;
    } else {
        lines = event->angleDelta().y() / kWheelNotch * kLinesPerNotch;
    }
    m_wheelRemainder += lines;
    const int whole = static_cast<int>(m_wheelRemainder);
    if (whole != 0) {
        m_wheelRemainder -= whole;
        emit scrolled(whole);
    }
    event->accept();
}

void TerminalView::geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry)
{
    QQuickPaintedItem::geometryChange(newGeometry, oldGeometry);
    updateGrid();
}

void TerminalView::updateGrid()
{
    const int columns = std::max(1, static_cast<int>(width() / m_cellWidth));
    const int rows = std::max(1, static_cast<int>(height() / m_cellHeight));
    if (columns != m_columns || rows != m_rows) {
        m_columns = columns;
        m_rows = rows;
        emit gridSizeChanged();
    }
}

void TerminalView::focusInEvent(QFocusEvent* event)
{
    QQuickPaintedItem::focusInEvent(event);
    update();
}

void TerminalView::focusOutEvent(QFocusEvent* event)
{
    QQuickPaintedItem::focusOutEvent(event);
    update();
}
