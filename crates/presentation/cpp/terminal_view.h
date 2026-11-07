#ifndef FINDERMAXXING_TERMINAL_VIEW_H
#define FINDERMAXXING_TERMINAL_VIEW_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

// Draws terminal frames that TerminalController (Rust) pushes in, and turns
// keys, text input, the mouse and the wheel into signals for it. No terminal
// logic lives here: no escape sequences, no colours, no scrollback.

#include <QtGui/QColor>
#include <QtGui/QFont>
#include <QtQml/qqmlregistration.h>
#include <QtQuick/QQuickPaintedItem>

#include <cstdint>
#include <vector>

#include "rust/cxx.h"
#include "presentation/src/rows_bridge.cxx.h"

class TerminalView : public QQuickPaintedItem
{
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(int columns READ columns NOTIFY gridSizeChanged)
    Q_PROPERTY(int rows READ rows NOTIFY gridSizeChanged)

public:
    explicit TerminalView(QQuickItem* parent = nullptr);

    void paint(QPainter* painter) override;

    // Called by Rust on the GUI thread.
    void setFrame(rust::Vec<TerminalRun> runs, TerminalCursor cursor, std::uint32_t background);

    int columns() const;
    int rows() const;

signals:
    void gridSizeChanged();
    // A key the terminal should see: Qt key code, modifiers, its text.
    void keyTyped(int key, int modifiers, const QString& text);
    // Text from an input method (accents, CJK), already composed.
    void textTyped(const QString& text);
    // Positive: back into history.
    void scrolled(int lines);
    void selecting(int fromRow, int fromColumn, int toRow, int toColumn);
    void selectionCleared();
    void copyRequested();
    void pasteRequested();

protected:
    void keyPressEvent(QKeyEvent* event) override;
    void inputMethodEvent(QInputMethodEvent* event) override;
    QVariant inputMethodQuery(Qt::InputMethodQuery query) const override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;
    void wheelEvent(QWheelEvent* event) override;
    void geometryChange(const QRectF& newGeometry, const QRectF& oldGeometry) override;
    void focusInEvent(QFocusEvent* event) override;
    void focusOutEvent(QFocusEvent* event) override;

private:
    struct Run
    {
        int row;
        int column;
        int cells;
        QString text;
        QColor foreground;
        QColor background;
        std::uint8_t style;
    };

    QPoint cellAt(QPointF position) const;
    void updateGrid();
    bool isShortcut(QKeyEvent* event, bool copy) const;

    QFont m_font;
    qreal m_cellWidth = 8;
    qreal m_cellHeight = 16;
    qreal m_ascent = 12;
    int m_columns = 80;
    int m_rows = 24;
    std::vector<Run> m_runs;
    TerminalCursor m_cursor { 0, 0, 3, 0 };
    QColor m_background;
    QPoint m_selectionStart;
    bool m_selecting = false;
    qreal m_wheelRemainder = 0;
};

#endif // FINDERMAXXING_TERMINAL_VIEW_H
