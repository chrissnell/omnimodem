// Authoritative JSC vectors from the REAL Qt jsc.cpp (JSC::compress/decompress),
// to confirm the Rust port's algorithm (not just the tables). ref: js8call jsc.cpp.
#include "jsc.h"
#include <cstdio>
#include <QString>
static QString bstr(Codeword const& b){ QString s; for(bool x: b) s += x?'1':'0'; return s; }
int main(){
    const char* msgs[] = {"HELLO WORLD","CQ CQ CQ DE K1ABC","THE QUICK BROWN FOX","ACK 73",
        "TESTING 123","STATUS OK","A","E","HELLO","WORLD","THIS IS A TEST MESSAGE FOR JS8","QSL DE W1AW","GM ES 73"};
    for(auto m: msgs){
        Codeword all; quint32 n=0;
        for(auto p: JSC::compress(QString(m))){ all += p.first; n += p.second; }
        QString round = JSC::decompress(all);
        printf("%s|%u|%s|%s\n", m, n, bstr(all).toUtf8().constData(), round.toUtf8().constData());
    }
    return 0;
}
