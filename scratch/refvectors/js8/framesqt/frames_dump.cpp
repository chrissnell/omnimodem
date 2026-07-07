#include "varicode.h"
#include <cstdio>
#include <QString>
static QString b64of(quint64 v){ return QString::number(v); }
int main(){
    // packCallsign / unpackCallsign (22-bit base-40)
    const char* calls[] = {"K1ABC","W1AW","VK3ABC","G0ABC","N5AC","AA1A","@CQ","@ALLCALL"};
    for(auto c: calls){ bool p=false; quint32 v=Varicode::packCallsign(QString(c),&p);
        printf("CALL %s %u %d\n", c, v, (int)p); }
    // packAlphaNumeric50 (50-bit compound)
    const char* comp[] = {"K1ABC","W1AW","VE3/K1ABC","K1ABC/P","@ALLCALL","N5AC"};
    for(auto c: comp){ quint64 v=Varicode::packAlphaNumeric50(QString(c));
        printf("ALPHA50 %s %s\n", c, b64of(v).toUtf8().constData()); }
    // packCompoundFrame(callsign,type,num,bits3) -> 12-char frame string
    { QString f=Varicode::packCompoundFrame("K1ABC", 0, 0, 0);
      printf("COMPOUND K1ABC 0 0 0 %s\n", f.toUtf8().constData()); }
    { QString f=Varicode::packCompoundFrame("VE3/K1ABC", 1, 1234, 5);
      printf("COMPOUND VE3/K1ABC 1 1234 5 %s\n", f.toUtf8().constData()); }
    // packDirectedMessage(text, mycall,...) -> frame string
    { QString to,cmd,num; bool tc=false; int n=0;
      QString f=Varicode::packDirectedMessage("W1AW SNR -5","K1ABC",&to,&tc,&cmd,&num,&n);
      printf("DIRECTED K1ABC|W1AW SNR -5 -> %s (to=%s cmd=%s num=%s n=%d)\n",
             f.toUtf8().constData(), to.toUtf8().constData(), cmd.toUtf8().constData(), num.toUtf8().constData(), n); }
    { QString to,cmd,num; bool tc=false; int n=0;
      QString f=Varicode::packDirectedMessage("W1AW SNR?","K1ABC",&to,&tc,&cmd,&num,&n);
      printf("DIRECTED K1ABC|W1AW SNR? -> %s (cmd=%s n=%d)\n",
             f.toUtf8().constData(), cmd.toUtf8().constData(), n); }
    return 0;
}
