#include "varicode.h"
// Verbatim from js8call/varicode.cpp:649-682.
QVector<bool> Varicode::intToBits(quint64 value, int expected){
    QVector<bool> bits;
    while(value){ bits.prepend((bool)(value & 1)); value = value >> 1; }
    if(expected){ while(bits.count() < expected){ bits.prepend((bool)0); } }
    return bits;
}
quint64 Varicode::bitsToInt(QVector<bool> const value){
    quint64 v = 0; foreach(bool bit, value){ v = (v << 1) + (int)(bit); } return v;
}
quint64 Varicode::bitsToInt(QVector<bool>::ConstIterator start, int n){
    quint64 v = 0; for(int i=0;i<n;i++){ int bit=(int)(*start); v=(v<<1)+(int)(bit); start++; } return v;
}
