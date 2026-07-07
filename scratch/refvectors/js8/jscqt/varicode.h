#ifndef VARICODE_MIN_H
#define VARICODE_MIN_H
#include <QVector>
#include <QtGlobal>
class Varicode {
public:
    static QVector<bool> intToBits(quint64 value, int expected=0);
    static quint64 bitsToInt(QVector<bool> const value);
    static quint64 bitsToInt(QVector<bool>::ConstIterator start, int n);
};
#endif
