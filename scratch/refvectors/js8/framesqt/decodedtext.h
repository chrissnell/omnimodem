#ifndef DECODEDTEXT_STUB_H
#define DECODEDTEXT_STUB_H
#include <QString>
#include <QtGlobal>
class DecodedText {
public:
    DecodedText(QString, int, int) {}
    QString message() const { return QString(); }
    quint8 frameType() const { return 0; }
};
#endif
