package work.premutation.bootah

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

object ApiCredentials {
    @Volatile var localToken:String? = null
    private const val alias = "bootah.api.token"
    fun newToken():String = ByteArray(32).also{SecureRandom().nextBytes(it)}.joinToString(""){"%02x".format(it)}
    private fun key():SecretKey {
        val store=KeyStore.getInstance("AndroidKeyStore").apply{load(null)}
        (store.getKey(alias,null) as? SecretKey)?.let{return it}
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES,"AndroidKeyStore").apply{
            init(KeyGenParameterSpec.Builder(alias,KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT).setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    private fun name(endpoint:String)=Base64.encodeToString(endpoint.toByteArray(),Base64.NO_WRAP)
    fun save(context:Context,endpoint:String,token:String) {
        val cipher=Cipher.getInstance("AES/GCM/NoPadding");cipher.init(Cipher.ENCRYPT_MODE,key())
        val payload=cipher.iv+cipher.doFinal(token.toByteArray(Charsets.UTF_8))
        context.getSharedPreferences("api_credentials",Context.MODE_PRIVATE).edit().putString(name(endpoint),Base64.encodeToString(payload,Base64.NO_WRAP)).apply()
    }
    fun remove(context:Context,endpoint:String) {context.getSharedPreferences("api_credentials",Context.MODE_PRIVATE).edit().remove(name(endpoint)).apply()}
    fun load(context:Context,endpoint:String):String? { return try {
        val stored=context.getSharedPreferences("api_credentials",Context.MODE_PRIVATE).getString(name(endpoint),null) ?: return null
        val payload=Base64.decode(stored,Base64.NO_WRAP);val cipher=Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE,key(),GCMParameterSpec(128,payload.copyOfRange(0,12)))
        String(cipher.doFinal(payload.copyOfRange(12,payload.size)),Charsets.UTF_8)
    } catch(_:Exception){null} }
}
